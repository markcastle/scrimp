//! The Explorer menu's COM class IDs, in one place, so the menu DLL and the
//! code that registers it can never disagree. Changing either breaks every
//! existing registration, so they never change.

/// "Scrimp Compress".
pub const COMPRESS_COMMAND: u128 = 0xf675_47c1_2d5f_491c_a3c3_df55_9dc9_612d;
/// "Scrimp Decompress".
pub const DECOMPRESS_COMMAND: u128 = 0x8981_5c1a_8e9b_4abe_888c_baf0_7d0d_b257;
/// "Scrimp Savings…", shown with Shift held.
pub const SAVINGS_COMMAND: u128 = 0xec07_cd5c_85c0_4589_8432_d26f_8591_1cbf;

/// A class ID in the form the registry uses: `{F67547C1-2D5F-491C-A3C3-DF559DC9612D}`.
pub fn braced(id: u128) -> String {
    let h = format!("{id:032X}");
    format!(
        "{{{}-{}-{}-{}-{}}}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_written_the_way_the_registry_expects() {
        assert_eq!(braced(COMPRESS_COMMAND), "{F67547C1-2D5F-491C-A3C3-DF559DC9612D}");
        assert_eq!(braced(DECOMPRESS_COMMAND), "{89815C1A-8E9B-4ABE-888C-BAF07D0DB257}");
        assert_eq!(braced(SAVINGS_COMMAND), "{EC07CD5C-85C0-4589-8432-D26F85911CBF}");
        assert_eq!(braced(1), "{00000000-0000-0000-0000-000000000001}");
    }
}
