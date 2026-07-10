pub(super) const GENERIC: &str = "\
- Prefer the smallest, local fix over a cross-file refactor.
- Search for existing patterns first; mirror naming, error handling, and conventions.
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and timeouts bounded.
- Make error handling explicit — don't silently ignore failures.
- Leave code warning-free and easy to verify.
- Don't add new dependencies without explicit user approval.
";

pub(super) const ZIG: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and buffers bounded.
- Handle errors explicitly with try/catch — avoid casual catch unreachable.
- Keep allocator ownership and lifetime clear.
- Prefer small, readable functions with minimal hidden control flow.
- Leave code formatted, buildable, and warning-free.
";

pub(super) const RUST: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and timeouts bounded.
- Use Result with meaningful error propagation — avoid unwrap() in non-test code.
- Keep async behavior bounded and timeouts explicit.
- Prefer small, focused changes over broad rewrites.
- Leave code clippy-clean with zero warnings.
";

pub(super) const TYPESCRIPT: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and timeouts bounded.
- Make error handling explicit — don't silently swallow rejections or errors.
- Use strict typing — avoid any unless justified.
- Keep async/Promise flows bounded and understandable.
- Leave typecheck and lint status clean.
";

pub(super) const C: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and buffer sizes bounded.
- Make error handling explicit — check return values.
- Keep pointer usage straightforward and well-scoped.
- Avoid preprocessor complexity when simpler code works.
- Leave build and test status clean.
";

pub(super) const GO: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and timeouts bounded.
- Check and propagate errors explicitly — don't ignore returned errors.
- Keep goroutine lifecycle and cancellation understandable.
- Prefer small functions and direct control flow.
- Leave formatting and vet status clean.
";

pub(super) const ELIXIR: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep retries and message flows bounded.
- Keep process and supervision boundaries clear.
- Handle {:ok, value} / {:error, reason} tuples explicitly.
- Avoid hiding important behavior in opaque control flow.
- Leave formatting and compilation warnings-free.
";

pub(super) const KOTLIN: &str = "\
- Keep control flow straightforward and easy to follow.
- Prefer val over var; keep mutation local and obvious.
- Treat nullability as part of the design — avoid !! outside tests or impossible states.
- Use structured concurrency; avoid GlobalScope and do not swallow CancellationException.
- Keep Gradle/Maven verification project-specific and use ./gradlew when available.
- Leave formatting, lint, and tests clean for the touched module.
";
