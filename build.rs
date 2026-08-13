fn main() {
    // `#[cfg(windows)]`, not a runtime env check: `embed-resource` is only
    // declared under `[target.'cfg(windows)'.build-dependencies]`, and that
    // predicate is evaluated against the *host* building build.rs, not the
    // `--target` being compiled for. A runtime check alone still requires
    // the crate to exist for every host this is ever built on — it doesn't,
    // e.g. on a Linux host cross-compiling nothing in particular. This was
    // caught by CI's native Linux job, not by cross-target `cargo check`
    // from Windows (which happens to have `embed-resource` available since
    // its host is Windows regardless of `--target`).
    #[cfg(windows)]
    {
        embed_resource::compile("assets/tray.rc", embed_resource::NONE);
    }
}
