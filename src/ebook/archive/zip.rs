use crate::ebook::archive::errors::ArchiveResult;
use crate::ebook::archive::{self, Archive, ArchiveError};
use crate::ebook::resource::Resource;
use crate::util::sync::Lock;
use std::io;
use std::io::{Read, Seek};
use std::path::Path;
use zip::ZipArchive as Zip;
use zip::read::ZipFile;

pub(crate) struct ZipArchive<R>(Lock<Zip<R>>);

impl<R: Read + Seek> ZipArchive<R> {
    /// `reader` (and optional `path` for a more descriptive error message).
    pub(crate) fn new(reader: R, path: Option<&Path>) -> ArchiveResult<Self> {
        Zip::new(reader)
            .and_then(|mut zip| {
                zip.validate_header_consistency()?;
                Ok(Self(Lock::new(zip)))
            })
            .map_err(|error| ArchiveError::UnreadableArchive {
                source: io::Error::from(error),
                path: path.map(Path::to_path_buf),
            })
    }

    fn get_file<'a>(archive: &'a mut Zip<R>, resource: &Resource) -> ArchiveResult<ZipFile<'a, R>> {
        archive
            .by_name(archive::extract_resource_key(resource)?)
            .map_err(|error| ArchiveError::InvalidResource {
                source: io::Error::from(error),
                resource: resource.as_static(),
            })
    }
}

impl<#[cfg(feature = "threadsafe")] R: Send + Sync, #[cfg(not(feature = "threadsafe"))] R> Archive
    for ZipArchive<R>
where
    R: Read + Seek + 'static,
{
    fn read_resource_bytes(&self, resource: &Resource) -> Result<Vec<u8>, ArchiveError> {
        let mut lock = acquire_archive_lock(&self.0)?;
        let mut zip_file = Self::get_file(&mut lock, resource)?;
        let mut buf = Vec::new();

        zip_file
            .read_to_end(&mut buf)
            .map(|_| buf)
            .map_err(|error| ArchiveError::CannotRead {
                source: error,
                resource: resource.as_static(),
            })
    }
}

#[cfg(feature = "threadsafe")]
fn acquire_archive_lock<T>(lock: &Lock<T>) -> ArchiveResult<std::sync::MutexGuard<'_, T>> {
    lock.lock().map_err(|_| ArchiveError::UnreadableArchive {
        source: io::Error::other("Poisoned ZipArchive"),
        path: None,
    })
}
#[cfg(not(feature = "threadsafe"))]
fn acquire_archive_lock<T>(lock: &Lock<T>) -> Result<std::cell::RefMut<'_, T>, ArchiveError> {
    Ok(lock.borrow_mut())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn rejects_conflicting_local_and_central_names() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("chapter.xhtml", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<p>chapter</p>").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert!(ZipArchive::new(Cursor::new(bytes.clone()), None).is_ok());

        let mut cloaked = bytes;
        assert_eq!(&cloaked[30..43], b"chapter.xhtml");
        cloaked[30] = b'x';
        let mut ordinary = Zip::new(Cursor::new(cloaked.clone())).unwrap();
        let mut content = String::new();
        ordinary
            .by_name("chapter.xhtml")
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "<p>chapter</p>");
        let error = ZipArchive::new(Cursor::new(cloaked), None)
            .err()
            .expect("conflicting names must be rejected");
        assert!(error.to_string().contains("filename mismatch"), "{error}");
    }
}
