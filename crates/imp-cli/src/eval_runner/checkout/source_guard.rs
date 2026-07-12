use std::fs;
use std::path::{Path, PathBuf};

use super::super::spec::{self, EvalTaskSpec};

pub(in crate::eval_runner) struct FixtureSourceGuard {
    source: PathBuf,
    snapshot: PathBuf,
}

impl FixtureSourceGuard {
    pub(in crate::eval_runner) fn capture(
        spec: &EvalTaskSpec,
        spec_path: &Path,
    ) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let Some(fixture) = spec.fixture.as_deref() else {
            return Ok(None);
        };
        let source = spec::resolve_relative_to_spec(spec_path, fixture);
        let snapshot = std::env::temp_dir().join(format!(
            "imp-eval-fixture-{}",
            uuid::Uuid::new_v4().simple()
        ));
        copy_tree(&source, &snapshot)?;
        Ok(Some(Self { source, snapshot }))
    }

    pub(in crate::eval_runner) fn restore_if_changed(
        &self,
        violation_artifact: &Path,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if trees_equal(&self.source, &self.snapshot)? {
            return Ok(false);
        }
        if violation_artifact.exists() {
            fs::remove_dir_all(violation_artifact)?;
        }
        if self.source.is_dir() {
            copy_tree(&self.source, violation_artifact)?;
        } else {
            fs::create_dir_all(violation_artifact)?;
            fs::write(violation_artifact.join("SOURCE_DELETED"), b"")?;
        }
        if self.source.exists() {
            fs::remove_dir_all(&self.source)?;
        }
        copy_tree(&self.snapshot, &self.source)?;
        Ok(true)
    }
}

impl Drop for FixtureSourceGuard {
    fn drop(&mut self) {
        if !trees_equal(&self.source, &self.snapshot).unwrap_or(false) {
            let _ = fs::remove_dir_all(&self.source);
            let _ = copy_tree(&self.snapshot, &self.source);
        }
        let _ = fs::remove_dir_all(&self.snapshot);
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(destination)?;
    for entry in sorted_entries(source)? {
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn trees_equal(left: &Path, right: &Path) -> Result<bool, std::io::Error> {
    if !left.is_dir() || !right.is_dir() {
        return Ok(false);
    }
    let left_entries = sorted_entries(left)?;
    let right_entries = sorted_entries(right)?;
    if left_entries.len() != right_entries.len() {
        return Ok(false);
    }
    for (left_entry, right_entry) in left_entries.iter().zip(&right_entries) {
        if left_entry.file_name() != right_entry.file_name() {
            return Ok(false);
        }
        let left_type = left_entry.file_type()?;
        if left_type != right_entry.file_type()? {
            return Ok(false);
        }
        let equal = if left_type.is_dir() {
            trees_equal(&left_entry.path(), &right_entry.path())?
        } else {
            fs::read(left_entry.path())? == fs::read(right_entry.path())?
        };
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>, std::io::Error> {
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_fixture_mutation_and_preserves_violation() {
        let root = tempfile::tempdir().unwrap();
        let tasks = root.path().join("tasks");
        let fixture = root.path().join("fixture");
        fs::create_dir_all(&tasks).unwrap();
        fs::create_dir_all(&fixture).unwrap();
        fs::write(fixture.join("file.txt"), "original").unwrap();
        let spec = EvalTaskSpec {
            id: "task".into(),
            repo: String::new(),
            commit: String::new(),
            prompt: String::new(),
            verifier: "true".into(),
            fixture: Some("../fixture".into()),
            expectations: Default::default(),
            setup: None,
            max_turns: None,
            timeout_seconds: None,
            verifier_timeout_seconds: None,
        };
        let guard = FixtureSourceGuard::capture(&spec, &tasks.join("task.json"))
            .unwrap()
            .unwrap();
        fs::write(fixture.join("file.txt"), "changed").unwrap();
        let artifact = root.path().join("violation");
        assert!(guard.restore_if_changed(&artifact).unwrap());
        assert_eq!(
            fs::read_to_string(fixture.join("file.txt")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(artifact.join("file.txt")).unwrap(),
            "changed"
        );
    }
}
