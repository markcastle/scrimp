//! scrimp-win on each kind of drive Phase 0 tested, using the small VHDX
//! drives scripts/test-volumes.ps1 makes.
//!
//! Each test pins what Windows actually does there, so a change in Windows
//! or in Scrimp that alters it is caught. Several show why Scrimp must check
//! the drive before trying to compress: the errors Windows gives on the
//! wrong kind of drive are misleading.
//!
//! Locally these skip unless the drives exist. On CI they never skip.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use common::{MB, file, test_drive, text};
use scrimp_core::{Algorithm, CompressionState, UnsupportedReason};
use scrimp_win::{DriveType, Error, compress, decompress, inspect, volume};

#[test]
fn a_mounted_vhdx_with_wof_attached_works_like_any_ntfs_drive() {
    let Some(dir) = test_drive("SCRIMP_TEST_NTFS4K") else {
        return;
    };
    let v = volume(dir.path()).unwrap();
    assert_eq!(
        (v.filesystem.as_str(), v.cluster_size, v.drive_type),
        ("NTFS", 4096, DriveType::Fixed),
        "{v:?}"
    );
    assert!(v.virtual_disk, "a mounted VHDX is a disk kept in a file: {v:?}");
    let system = volume(std::path::Path::new(&std::env::var("SystemDrive").unwrap())).unwrap();
    assert!(!system.virtual_disk, "the system drive isn't: {system:?}");

    let data = text(2 * MB);
    for algorithm in Algorithm::ALL {
        assert_eq!(v.unsupported(algorithm), None);
        let p = file(dir.path(), &format!("{}.txt", algorithm.cli_name()), &data);
        let info = compress(&p, algorithm).unwrap();
        assert_eq!(info.state, CompressionState::Compressed(algorithm));
        assert!(info.on_disk < info.logical);
        assert_eq!(decompress(&p).unwrap().state, CompressionState::Uncompressed);
        assert_eq!(fs::read(&p).unwrap(), data);
    }
}

#[test]
fn sixty_four_kilobyte_clusters_allow_wof_but_not_ntfs_compression() {
    let Some(dir) = test_drive("SCRIMP_TEST_NTFS64K") else {
        return;
    };
    let v = volume(dir.path()).unwrap();
    assert_eq!(v.cluster_size, 65536, "{v:?}");
    assert_eq!(
        v.unsupported(Algorithm::Lznt1),
        Some(UnsupportedReason::ClusterTooLarge)
    );
    assert_eq!(v.unsupported(Algorithm::Lzx), None);

    let data = text(2 * MB);
    let p = file(dir.path(), "Example.dat", &data);
    assert_eq!(
        compress(&p, Algorithm::Lzx).unwrap().state,
        CompressionState::Compressed(Algorithm::Lzx)
    );
    decompress(&p).unwrap();
    // Windows says "Incorrect function", which from an NTFS call means "not supported".
    assert_eq!(compress(&p, Algorithm::Lznt1), Err(Error::NotSupported));
    assert_eq!(inspect(&p).unwrap().state, CompressionState::Uncompressed);
    assert_eq!(fs::read(&p).unwrap(), data);
}

#[test]
fn exfat_supports_nothing_and_its_errors_are_misleading() {
    let Some(dir) = test_drive("SCRIMP_TEST_EXFAT") else {
        return;
    };
    let v = volume(dir.path()).unwrap();
    assert_eq!(v.filesystem, "exFAT", "{v:?}");
    for algorithm in Algorithm::ALL {
        assert_eq!(
            v.unsupported(algorithm),
            Some(UnsupportedReason::FileSystem("exFAT".into()))
        );
    }

    let data = text(MB);
    let p = file(dir.path(), "Example.dat", &data);
    assert_eq!(inspect(&p).unwrap().state, CompressionState::Uncompressed);
    // exFAT has no WOF, and the error is the same one an NTFS drive without
    // WOF attached gives. Only the volume check tells them apart.
    assert_eq!(compress(&p, Algorithm::Lzx), Err(Error::WofUnavailable));
    assert_eq!(compress(&p, Algorithm::Lznt1), Err(Error::NotSupported));
    assert_eq!(fs::read(&p).unwrap(), data);
}

#[test]
fn refs_answers_wof_with_would_not_save_space() {
    let Some(dir) = test_drive("SCRIMP_TEST_REFS") else {
        return;
    };
    let v = volume(dir.path()).unwrap();
    assert_eq!(v.filesystem, "ReFS", "{v:?}");
    for algorithm in Algorithm::ALL {
        assert_eq!(
            v.unsupported(algorithm),
            Some(UnsupportedReason::FileSystem("ReFS".into()))
        );
    }

    let data = text(MB);
    let p = file(dir.path(), "Example.dat", &data);
    assert_eq!(inspect(&p).unwrap().state, CompressionState::Uncompressed);
    // The trap: very compressible text, and Windows says it wouldn't save
    // space. Trusting this would make a Dev Drive look incompressible.
    assert_eq!(compress(&p, Algorithm::Lzx), Err(Error::NotBeneficial));
    assert_eq!(compress(&p, Algorithm::Lznt1), Err(Error::NotSupported));
    assert_eq!(fs::read(&p).unwrap(), data);
}

#[test]
fn without_wof_attached_only_ntfs_compression_works() {
    let Some(dir) = test_drive("SCRIMP_TEST_NO_WOF") else {
        return;
    };
    let v = volume(dir.path()).unwrap();
    // The drive itself looks fine: only trying WOF shows the driver is missing.
    assert_eq!(v.unsupported(Algorithm::Lzx), None, "{v:?}");

    let data = text(MB);
    let p = file(dir.path(), "Example.dat", &data);
    for algorithm in [Algorithm::Xpress4k, Algorithm::Lzx] {
        assert_eq!(compress(&p, algorithm), Err(Error::WofUnavailable), "{algorithm}");
    }
    assert_eq!(
        compress(&p, Algorithm::Lznt1).unwrap().state,
        CompressionState::Compressed(Algorithm::Lznt1)
    );
    assert_eq!(decompress(&p).unwrap().state, CompressionState::Uncompressed);
    assert_eq!(fs::read(&p).unwrap(), data);
}
