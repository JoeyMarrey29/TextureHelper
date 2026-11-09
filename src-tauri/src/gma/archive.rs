use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

pub struct GmaArchive {
    path: PathBuf,
}

impl GmaArchive {
    pub fn open<P>(path: P) -> io::Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref().to_path_buf();

        if !path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("GMA not found: {}", path.display()),
            ));
        }

        /*
            Validate that the archive can actually
            be opened, but don't keep it loaded.
        */

        let _archive = gma::open(&path).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Could not open GMA: {}", error),
            )
        })?;

        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn contains(&self, requested_path: &str) -> io::Result<bool> {
        let archive = gma::open(&self.path).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Could not open GMA {}: {}", self.path.display(), error),
            )
        })?;

        let wanted = normalize_path(requested_path);

        for entry in archive.entries() {
            let entry_path = normalize_path(entry.filename());

            if entry_path == wanted {
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn paths(&self) -> io::Result<Vec<String>> {
        let archive = gma::open(&self.path).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Could not open GMA {}: {}", self.path.display(), error),
            )
        })?;

        Ok(archive
            .entries()
            .map(|entry| entry.filename().to_string())
            .collect())
    }

    pub fn find_path(&self, requested_path: &str) -> io::Result<Option<String>> {
        let archive = gma::open(&self.path).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Could not open GMA {}: {}", self.path.display(), error),
            )
        })?;

        let wanted = normalize_path(requested_path);

        for entry in archive.entries() {
            let path = entry.filename().to_string();

            if normalize_path(&path) == wanted {
                return Ok(Some(path));
            }
        }

        Ok(None)
    }

    pub fn read(&self, requested_path: &str) -> io::Result<Vec<u8>> {
        let mut archive = gma::open(&self.path).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Could not open GMA {}: {}", self.path.display(), error),
            )
        })?;

        let wanted = normalize_path(requested_path);

        /*
            Find the matching entry index first.

            We use an index rather than holding
            a borrowed FileEntry while also
            mutably borrowing the archive.
        */

        let entry_index = archive
            .entries()
            .enumerate()
            .find_map(|(index, entry)| {
                if normalize_path(entry.filename()) == wanted {
                    Some(index)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("Asset not found in GMA: {}", requested_path),
                )
            })?;

        /*
            Re-fetch the entry by index.

            This should only read the requested
            file's payload, not the whole 3.29 GB
            archive into RAM.
        */

        let entry = archive
            .entries()
            .nth(entry_index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "GMA entry disappeared"))?
            .clone();

        let result = archive
            .read_entry(&entry, |_, reader| {
                let mut bytes = Vec::new();

                reader.read_to_end(&mut bytes)?;

                Ok::<Vec<u8>, io::Error>(bytes)
            })
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Could not read GMA entry: {}", error),
                )
            })?;

        /*
            Depending on the crate's callback API,
            the callback's io::Result may be nested.
        */

        result.map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Could not extract GMA entry: {}", error),
            )
        })
    }
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .to_lowercase()
}
