//! Bounded, content-validated local images and the session-owned copies of them.
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Base64 must fit inside a 16 MiB IPC frame.
pub const MAX_IMAGE: u64 = 11 * 1024 * 1024;

fn invalid(message: impl ToString) -> Error {
    Error::Invalid(message.to_string())
}

/// Read a regular file without trusting its extension, and fully decode it before returning
/// any bytes. Decode limits also bound images with tiny compressed files and huge dimensions.
pub fn read_image(path: &Path) -> Result<(Vec<u8>, &'static str)> {
    if !path.is_absolute() {
        return Err(invalid("An image needs an absolute path"));
    }
    if !std::fs::metadata(path).map_err(|_| invalid("This image is no longer there"))?.is_file() {
        return Err(invalid("This file is not an image"));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // Avoid a path swapped for a FIFO blocking the daemon before the handle is checked.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NONBLOCK);
    }
    let mut file = options.open(path).map_err(|_| invalid("This image is no longer there"))?;
    let metadata = file.metadata().map_err(invalid)?;
    if !metadata.is_file() {
        return Err(invalid("This file is not an image"));
    }
    if metadata.len() > MAX_IMAGE {
        return Err(invalid("This image is over 11 MB"));
    }
    let mut bytes = Vec::new();
    file.by_ref().take(MAX_IMAGE + 1).read_to_end(&mut bytes).map_err(invalid)?;
    if bytes.len() as u64 > MAX_IMAGE {
        return Err(invalid("This image is over 11 MB"));
    }
    // WebKit shows these but the decoder crate can't read all of them: check their structure.
    if is_heic(&bytes) {
        return Ok((bytes, "image/heic"));
    }
    if is_tiff(&bytes) {
        return Ok((bytes, "image/tiff"));
    }
    let format = image::guess_format(&bytes).map_err(|_| invalid("This file is not a supported image"))?;
    let mime = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Gif => "image/gif",
        image::ImageFormat::WebP => "image/webp",
        image::ImageFormat::Bmp => "image/bmp",
        _ => return Err(invalid("This file is not a supported image")),
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE as u32);
    limits.max_image_height = Some(MAX_SIDE as u32);
    // Room for a tall full-page Retina screenshot (2880 x 20000 RGBA is about 230 MiB).
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|_| invalid("This file is not a supported image"))?;
    Ok((bytes, mime))
}

/// The largest width or height accepted, decoded or not.
const MAX_SIDE: u64 = 16384;

/// A HEIC file: ISO media boxes that exactly fill the file, starting with `ftyp` naming an HEVC
/// image brand (which leaves out AVIF), and holding the image's `meta` box.
fn is_heic(bytes: &[u8]) -> bool {
    const BRANDS: [&[u8]; 8] = [b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"hevm", b"hevs"];
    let mut rest = bytes;
    let (mut first, mut meta) = (true, false);
    while !rest.is_empty() {
        let Some((kind, body, after)) = iso_box(rest) else { return false };
        if first {
            // The major brand, the minor version, then the compatible brands.
            let branded = kind == b"ftyp"
                && body.len() >= 8
                && (body[8..].chunks_exact(4).chain(body[..4].chunks_exact(4))).any(|brand| BRANDS.contains(&brand));
            if !branded {
                return false;
            }
        }
        first = false;
        meta |= kind == b"meta";
        rest = after;
    }
    meta
}

/// One ISO media box of `bytes`: its type, its contents and what follows it.
fn iso_box(bytes: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let size = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as u64;
    let kind = bytes.get(4..8)?;
    let (start, end) = match size {
        0 => (8, bytes.len() as u64),
        1 => (16, u64::from_be_bytes(bytes.get(8..16)?.try_into().ok()?)),
        size => (8, size),
    };
    if end < start || end > bytes.len() as u64 {
        return None;
    }
    Some((kind, &bytes[start as usize..end as usize], &bytes[end as usize..]))
}

/// A TIFF file, classic or BigTIFF: its first directory lies inside the file and names a width
/// and a height within the limit, and where the picture's data is.
fn is_tiff(bytes: &[u8]) -> bool {
    tiff_directory(bytes).is_some_and(|(width, height, data)| {
        (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height) && data
    })
}

fn tiff_directory(bytes: &[u8]) -> Option<(u64, u64, bool)> {
    let big_endian = match bytes.get(..2)? {
        b"II" => false,
        b"MM" => true,
        _ => return None,
    };
    // An unsigned number of `size` bytes at `at`, in the file's byte order.
    let number = |at: usize, size: usize| -> Option<u64> {
        let field = bytes.get(at..at.checked_add(size)?)?;
        let fold = |value: u64, byte: &u8| value << 8 | u64::from(*byte);
        Some(if big_endian { field.iter().fold(0, fold) } else { field.iter().rev().fold(0, fold) })
    };
    let (directory, count_size, entry_size, inline) = match number(2, 2)? {
        42 => (number(4, 4)?, 2, 12, 4),
        43 if number(4, 2)? == 8 && number(6, 2)? == 0 => (number(8, 8)?, 8, 20, 8),
        _ => return None,
    };
    let directory = usize::try_from(directory).ok()?;
    let entries = usize::try_from(number(directory, count_size)?).ok()?;
    let (mut width, mut height, mut data) = (0, 0, false);
    for index in 0..entries {
        let entry = directory.checked_add(count_size)?.checked_add(index.checked_mul(entry_size)?)?;
        let value = entry + 2 + 2 + inline;
        let side = match number(entry + 2, 2)? {
            3 => number(value, 2),
            4 => number(value, 4),
            16 => number(value, 8),
            _ => None,
        };
        match number(entry, 2)? {
            256 => width = side?,
            257 => height = side?,
            273 | 324 => data = true,
            _ => {}
        }
    }
    Some((width, height, data))
}

/// Stable mapping from an agent's path to its session's copy. The agent never chooses a
/// destination file name; the path hash also prevents equal item ids in workers colliding.
pub fn retained_path(directory: &Path, source: &Path) -> PathBuf {
    directory.join(format!("path-{}", blake3::hash(source.as_os_str().as_encoded_bytes()).to_hex()))
}

/// Copy before recording the item. Store validated bytes, not a second read of the source.
/// A file re-created at the same path replaces the copy; once the original has disappeared,
/// the existing copy stays usable.
pub fn retain_image(directory: &Path, source: &Path) -> Result<PathBuf> {
    if !source.is_absolute() {
        return Err(invalid("An image needs an absolute path"));
    }
    let target = retained_path(directory, source);
    let bytes = match read_image(source) {
        Ok((bytes, _)) => bytes,
        Err(_) if target.is_file() => return Ok(target),
        Err(error) => return Err(error),
    };
    std::fs::create_dir_all(directory).map_err(invalid)?;
    let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = std::fs::write(&temporary, bytes).and_then(|()| std::fs::rename(&temporary, &target));
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result.map_err(invalid)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1×1 PNG.
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21,
        196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 100, 248, 207, 80, 15, 0, 3, 134, 1, 128, 90, 52, 125,
        107, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("thread-image-files-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// The reply for `bytes` in a file whose name says nothing about what it holds.
    fn read_bytes(bytes: &[u8]) -> Result<&'static str> {
        let folder = folder("bytes");
        let path = folder.join("file");
        std::fs::write(&path, bytes).unwrap();
        let read = read_image(&path).map(|(_, mime)| mime);
        std::fs::remove_dir_all(folder).unwrap();
        read
    }

    /// A classic TIFF with a directory of `(tag, type, value)` right after its header.
    fn tiff(big_endian: bool, entries: &[(u16, u16, u32)]) -> Vec<u8> {
        let two = |value: u16| if big_endian { value.to_be_bytes() } else { value.to_le_bytes() };
        let four = |value: u32| if big_endian { value.to_be_bytes() } else { value.to_le_bytes() };
        let mut bytes = if big_endian { b"MM".to_vec() } else { b"II".to_vec() };
        bytes.extend(two(42));
        bytes.extend(four(8));
        bytes.extend(two(entries.len() as u16));
        for (tag, kind, value) in entries {
            bytes.extend(two(*tag));
            bytes.extend(two(*kind));
            bytes.extend(four(1));
            match kind {
                3 => {
                    bytes.extend(two(*value as u16));
                    bytes.extend([0, 0]);
                }
                _ => bytes.extend(four(*value)),
            }
        }
        bytes
    }

    fn big_tiff(entries: &[(u16, u16, u64)]) -> Vec<u8> {
        let mut bytes = b"II".to_vec();
        bytes.extend(43u16.to_le_bytes());
        bytes.extend(8u16.to_le_bytes());
        bytes.extend(0u16.to_le_bytes());
        bytes.extend(16u64.to_le_bytes());
        bytes.extend((entries.len() as u64).to_le_bytes());
        for (tag, kind, value) in entries {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(kind.to_le_bytes());
            bytes.extend(1u64.to_le_bytes());
            bytes.extend(value.to_le_bytes());
        }
        bytes
    }

    fn iso_media_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = (body.len() as u32 + 8).to_be_bytes().to_vec();
        bytes.extend(kind);
        bytes.extend(body);
        bytes
    }

    fn heic(ftyp: &[u8]) -> Vec<u8> {
        [iso_media_box(b"ftyp", ftyp), iso_media_box(b"meta", b"items"), iso_media_box(b"mdat", b"picture")].concat()
    }

    #[test]
    fn a_real_image_is_read_by_what_it_holds_not_its_name() {
        assert_eq!(read_bytes(PNG).unwrap(), "image/png");
    }

    #[test]
    fn a_file_that_is_not_an_image_is_refused_whatever_its_name() {
        let folder = folder("not-image");
        let path = folder.join("notes.png");
        std::fs::write(&path, "Text with an image's extension.").unwrap();
        assert!(read_image(&path).unwrap_err().to_string().contains("not a supported image"));
        std::fs::remove_dir_all(folder).unwrap();
        // An image's first bytes alone are not an image.
        assert!(read_bytes(&PNG[..20]).is_err());
    }

    #[test]
    fn an_oversized_file_is_refused() {
        let folder = folder("oversized");
        let path = folder.join("huge.png");
        std::fs::write(&path, PNG).unwrap();
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(MAX_IMAGE + 1).unwrap();
        assert!(read_image(&path).unwrap_err().to_string().contains("over 11 MB"));
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_missing_file_a_folder_and_a_relative_path_are_refused() {
        let folder = folder("missing");
        assert!(read_image(&folder.join("gone.png")).unwrap_err().to_string().contains("no longer there"));
        assert!(read_image(&folder).unwrap_err().to_string().contains("not an image"));
        assert!(read_image(Path::new("picture.png")).unwrap_err().to_string().contains("absolute path"));
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn tiff_files_of_either_byte_order_and_bigtiff_are_accepted() {
        let entries = [(256, 4, 3000), (257, 3, 2000), (273, 4, 8)];
        assert_eq!(read_bytes(&tiff(false, &entries)).unwrap(), "image/tiff");
        assert_eq!(read_bytes(&tiff(true, &entries)).unwrap(), "image/tiff");
        assert_eq!(read_bytes(&big_tiff(&[(256, 16, 3000), (257, 4, 2000), (324, 16, 16)])).unwrap(), "image/tiff");
    }

    #[test]
    fn a_fake_tiff_is_refused() {
        assert!(read_bytes(b"II*\0not a directory").is_err());
        // Its directory lies outside the file.
        assert!(read_bytes(&[b"II*\0".as_slice(), &9000u32.to_le_bytes()].concat()).is_err());
        // No width, no height, no picture data, and sides over the limit.
        assert!(read_bytes(&tiff(false, &[(257, 4, 2000), (273, 4, 8)])).is_err());
        assert!(read_bytes(&tiff(false, &[(256, 4, 3000), (273, 4, 8)])).is_err());
        assert!(read_bytes(&tiff(false, &[(256, 4, 3000), (257, 4, 2000)])).is_err());
        assert!(read_bytes(&tiff(false, &[(256, 4, 16385), (257, 4, 2000), (273, 4, 8)])).is_err());
    }

    #[test]
    fn heic_files_are_accepted_by_a_major_or_a_compatible_brand() {
        assert_eq!(read_bytes(&heic(b"heic\0\0\0\0mif1heic")).unwrap(), "image/heic");
        assert_eq!(read_bytes(&heic(b"mif1\0\0\0\0mif1heix")).unwrap(), "image/heic");
    }

    #[test]
    fn a_fake_heic_is_refused() {
        // Boxes that run past the file's end.
        assert!(read_bytes(b"\0\0\0\x18ftypheic\0\0\0\0heicmif1 and then text").is_err());
        // No `meta` box, or another format with the same container.
        assert!(read_bytes(&iso_media_box(b"ftyp", b"heic\0\0\0\0mif1heic")).is_err());
        assert!(read_bytes(&heic(b"avif\0\0\0\0mif1avif")).is_err());
        // `ftyp` is not first.
        assert!(read_bytes(&[iso_media_box(b"meta", b"items"), iso_media_box(b"ftyp", b"heic\0\0\0\0")].concat()).is_err());
    }

    #[test]
    fn a_kept_copy_survives_deleting_its_source_for_every_format() {
        let entries = [(256, 4, 3000), (257, 3, 2000), (273, 4, 8)];
        let formats = [(PNG.to_vec(), "image/png"), (tiff(false, &entries), "image/tiff"), (heic(b"heic\0\0\0\0heic"), "image/heic")];
        let folder = folder("kept");
        let storage = folder.join("session/thread-images");
        for (index, (bytes, mime)) in formats.iter().enumerate() {
            let source = folder.join(format!("viewed-{index}"));
            std::fs::write(&source, bytes).unwrap();
            let kept = retain_image(&storage, &source).unwrap();
            std::fs::remove_file(&source).unwrap();
            assert_eq!(kept, retained_path(&storage, &source));
            assert_eq!(read_image(&kept).unwrap(), (bytes.clone(), *mime));
            // Recording the same path again keeps the copy.
            assert_eq!(retain_image(&storage, &source).unwrap(), kept);
            assert_eq!(read_image(&kept).unwrap().0, *bytes);
        }
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_file_recreated_at_the_same_path_replaces_the_copy() {
        let folder = folder("recreated");
        let storage = folder.join("thread-images");
        let source = folder.join("shot");
        std::fs::write(&source, PNG).unwrap();
        retain_image(&storage, &source).unwrap();
        let newer = tiff(false, &[(256, 4, 1), (257, 4, 1), (273, 4, 8)]);
        std::fs::write(&source, &newer).unwrap();
        let kept = retain_image(&storage, &source).unwrap();
        assert_eq!(read_image(&kept).unwrap().0, newer);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn nothing_is_kept_of_a_refused_file() {
        let folder = folder("refused");
        let storage = folder.join("thread-images");
        let source = folder.join("notes.png");
        std::fs::write(&source, "text").unwrap();
        assert!(retain_image(&storage, &source).is_err());
        assert!(retain_image(&storage, &folder.join("gone.png")).is_err());
        assert!(retain_image(&storage, Path::new("picture.png")).is_err());
        assert!(!storage.exists());
        std::fs::remove_dir_all(folder).unwrap();
    }
}
