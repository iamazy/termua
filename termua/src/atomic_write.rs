use std::{io::Write, path::Path};

use anyhow::Context;

pub fn write_file(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    // Be explicit about preserving metadata when overwriting sensitive files (e.g. known_hosts).
    #[cfg(unix)]
    let opts = {
        let mut opts = atomic_write_file::AtomicWriteFile::options();
        use atomic_write_file::unix::OpenOptionsExt;
        opts.preserve_mode(true);
        opts.try_preserve_owner(true);
        opts
    };

    #[cfg(not(unix))]
    let opts = atomic_write_file::AtomicWriteFile::options();

    let mut file = opts
        .open(path)
        .with_context(|| format!("open {path:?} for atomic write"))?;
    file.write_all(contents)
        .with_context(|| format!("write {path:?} atomically"))?;
    file.commit()
        .with_context(|| format!("commit atomic write for {path:?}"))?;
    Ok(())
}

pub fn write_string(path: &Path, contents: &str) -> anyhow::Result<()> {
    write_file(path, contents.as_bytes())
}
