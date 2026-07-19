use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use sha1::{Digest, Sha1};

/// Encode Git objects into a version-2 packfile.
pub fn encode_packfile(objects: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PACK");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(objects.len() as u32).to_be_bytes());

    for (obj_type, payload) in objects {
        write_object_header(&mut out, *obj_type, payload.len());

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).expect("writing to in-memory zlib encoder cannot fail");
        let compressed = encoder.finish().expect("finishing in-memory zlib encoder cannot fail");
        out.extend_from_slice(&compressed);
    }

    let mut hasher = Sha1::new();
    hasher.update(&out);
    let checksum = hasher.finalize();
    out.extend_from_slice(&checksum[..20]);

    out
}

fn write_object_header(out: &mut Vec<u8>, obj_type: u8, size: usize) {
    let mut remaining = size >> 4;
    let mut first = ((obj_type & 0x07) << 4) | ((size & 0x0f) as u8);
    if remaining > 0 {
        first |= 0x80;
    }
    out.push(first);

    while remaining > 0 {
        let mut next = (remaining & 0x7f) as u8;
        remaining >>= 7;
        if remaining > 0 {
            next |= 0x80;
        }
        out.push(next);
    }
}

#[cfg(test)]
mod tests {
    use super::encode_packfile;

    #[test]
    fn packfile_has_expected_header_prefix() {
        let objects = vec![(3u8, b"hello".as_slice())];
        let pack = encode_packfile(&objects);

        assert!(pack.starts_with(b"PACK\0\0\0\x02"), "packfile must start with PACK + v2 header");
        assert_eq!(&pack[8..12], &1u32.to_be_bytes());
        assert!(pack.len() > 12 + 20, "pack must include body and checksum");
    }
}
