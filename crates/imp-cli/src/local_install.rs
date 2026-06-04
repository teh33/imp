use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

fn split_path_entries(path: Option<OsString>) -> Vec<PathBuf> {
    path.as_deref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .collect()
}

fn find_imp_on_path_from(path: Option<OsString>) -> Option<PathBuf> {
    split_path_entries(path)
        .into_iter()
        .map(|dir| dir.join("imp"))
        .find(|candidate| candidate.is_file())
}

fn path_contains_dir(path: Option<OsString>, dir: &Path) -> bool {
    split_path_entries(path)
        .into_iter()
        .any(|entry| entry == dir)
}

fn preferred_user_install_path(home: &Path, path: Option<OsString>) -> PathBuf {
    let home_bin = home.join("bin");
    if path_contains_dir(path.clone(), &home_bin) {
        return home_bin.join("imp");
    }

    let local_bin = home.join(".local/bin");
    if path_contains_dir(path.clone(), &local_bin) {
        return local_bin.join("imp");
    }

    home.join(".cargo/bin/imp")
}

pub(crate) fn resolve_install_destination(
    home: &Path,
    path: Option<OsString>,
    active_imp: Option<PathBuf>,
    dest_override: Option<PathBuf>,
) -> PathBuf {
    if let Some(dest) = dest_override {
        return dest;
    }

    if let Some(active) = active_imp {
        if active.starts_with(home) {
            return active;
        }
    }

    preferred_user_install_path(home, path)
}

fn install_binary_to(source: &Path, dest: &Path) -> io::Result<()> {
    let parent = dest.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Install destination has no parent: {}", dest.display()),
        )
    })?;
    std::fs::create_dir_all(parent)?;

    let temp = dest.with_extension("tmp");
    std::fs::copy(source, &temp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&temp, perms)?;
    }
    std::fs::rename(&temp, dest)?;
    Ok(())
}

pub(crate) fn run_install_local(
    dest_override: Option<PathBuf>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let current_exe = std::env::current_exe()?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let path_env = std::env::var_os("PATH");
    let active_imp = find_imp_on_path_from(path_env.clone());
    let dest = resolve_install_destination(&home, path_env, active_imp.clone(), dest_override);

    if dry_run {
        println!("{}", dest.display());
        return Ok(());
    }

    install_binary_to(&current_exe, &dest)?;

    println!("Installed imp to {}", dest.display());
    if let Some(previous) = active_imp {
        if previous != dest {
            println!(
                "Updated active-user install target instead of Cargo bin shadow path. Previous `imp` path was {}.",
                previous.display()
            );
        }
    }

    let resolved_after = find_imp_on_path_from(std::env::var_os("PATH"));
    match resolved_after {
        Some(path) if path == dest => {
            println!("`imp` now resolves to {}", path.display());
        }
        Some(path) => {
            println!(
                "Installed to {}, but `imp` still resolves to {}. Adjust PATH or rerun with --dest {}.",
                dest.display(),
                path.display(),
                path.display()
            );
        }
        None => {
            println!(
                "Installed to {}, but `imp` is not currently on PATH. Add {} to PATH.",
                dest.display(),
                dest.parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
        }
    }

    Ok(())
}
