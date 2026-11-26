//! Whether there is room to decompress, before starting.
//!
//! Decompressing makes files take their full size again, so it needs free
//! space. Scrimp checks before it starts, and keeps a reserve so it never
//! leaves a drive (above all the system drive) completely full. The check is
//! repeated between files, because other programs use space too.

use crate::report::format_size;

const GIB: u64 = 1024 * 1024 * 1024;

/// The space Scrimp leaves free: the larger of 1 GB and 2% of the drive.
pub fn reserve(drive_size: u64) -> u64 {
    GIB.max(drive_size / 50)
}

/// Not enough room to decompress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortfall {
    /// Extra space the files need once decompressed.
    pub needed: u64,
    /// Free space on the drive now.
    pub free: u64,
    /// Space Scrimp keeps free.
    pub reserve: u64,
}

impl Shortfall {
    /// What to tell the person, in plain words.
    pub fn message(&self) -> String {
        format!(
            "Decompressing needs {} more space, but only {} is free. Scrimp always leaves {} free so the drive never fills up.",
            format_size(self.needed),
            format_size(self.free),
            format_size(self.reserve)
        )
    }
}

/// Checks whether files that will grow by `needed` bytes fit on a drive of
/// `drive_size` bytes with `free` bytes free, leaving the reserve.
pub fn check_room(needed: u64, free: u64, drive_size: u64) -> Result<(), Shortfall> {
    let reserve = reserve(drive_size);
    if free.saturating_sub(reserve) >= needed {
        Ok(())
    } else {
        Err(Shortfall { needed, free, reserve })
    }
}

/// How much more space a compressed file needs once decompressed.
pub fn growth(logical: u64, on_disk: u64) -> u64 {
    logical.saturating_sub(on_disk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserve_is_one_gigabyte_or_two_percent() {
        assert_eq!(reserve(10 * GIB), GIB);
        assert_eq!(reserve(500 * GIB), 10 * GIB);
        assert_eq!(reserve(0), GIB);
    }

    #[test]
    fn room_is_checked_against_free_space_less_the_reserve() {
        let drive = 100 * GIB;
        // 30 GB free, less the 2 GB reserve, leaves 28 GB.
        assert_eq!(check_room(28 * GIB, 30 * GIB, drive), Ok(()));
        assert_eq!(
            check_room(30 * GIB, 30 * GIB, drive),
            Err(Shortfall {
                needed: 30 * GIB,
                free: 30 * GIB,
                reserve: 2 * GIB
            })
        );
        assert!(
            check_room(1, 0, drive).is_err(),
            "an almost full drive never underflows"
        );
        assert_eq!(check_room(0, 0, drive), Ok(()), "nothing to grow always fits");
    }

    #[test]
    fn a_shortfall_explains_itself() {
        let s = check_room(30 * GIB, 12 * GIB, 100 * GIB).unwrap_err();
        assert_eq!(
            s.message(),
            "Decompressing needs 30.0 GB more space, but only 12.0 GB is free. Scrimp always leaves 2.0 GB free so the drive never fills up."
        );
    }

    #[test]
    fn growth_is_the_difference_and_never_negative() {
        assert_eq!(growth(86, 51), 35);
        assert_eq!(growth(10, 12), 0, "a file can take more than its size on disk");
    }
}
