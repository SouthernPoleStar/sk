//! `vendor_boot` images, header version 3 or 4: on phones that keep their
//! recovery out of a `recovery` partition, the ramdisk that holds it and the
//! device tree the bootloader hands the kernel are here, and there is no
//! kernel of the image's own.
//!
//! The image is a header, the ramdisk (in version 4, as many fragments as its
//! table names), the device tree, that table and the boot config, each of
//! them starting on a page boundary.

use anyhow::{bail, ensure, Context, Result};
use std::fs;
use std::path::Path;

/// What a vendor_boot image starts with.
pub const MAGIC: &[u8; 8] = b"VNDRBOOT";

const CMDLINE_LEN: usize = 2048;
const NAME_LEN: usize = 16;
const ENTRY_NAME_LEN: usize = 32;
const BOARD_ID: usize = 16;
/// The fields of one table entry: its size, offset and type, its name and
/// the board IDs it is for.
const ENTRY_LEN: usize = 12 + ENTRY_NAME_LEN + BOARD_ID * 4;

/// One ramdisk of a version 4 image: the bootloader loads each where the
/// table says, and the kernel unpacks them one after another.
#[derive(Clone)]
pub struct Fragment {
    pub name: String,
    /// What the fragment holds: 1 the platform's, 2 the recovery's, 3 a
    /// vendor dlkm's.
    pub kind: u32,
    pub board_id: [u32; BOARD_ID],
    pub data: Vec<u8>,
}

/// A vendor_boot image split into its header, ramdisks, device tree and boot
/// config.
pub struct VendorBoot {
    pub header_version: u32,
    pub page_size: u32,
    pub kernel_addr: u32,
    pub ramdisk_addr: u32,
    /// The kernel command line the bootloader adds to its own.
    pub cmdline: Vec<u8>,
    pub tags_addr: u32,
    pub name: Vec<u8>,
    pub dtb_addr: u64,
    pub fragments: Vec<Fragment>,
    pub dtb: Vec<u8>,
    pub bootconfig: Vec<u8>,
}

/// Whether `bytes` start a vendor_boot image.
pub fn is_vendor_boot(bytes: &[u8]) -> bool {
    bytes.len() >= MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    let b = bytes.get(at..at + 4).with_context(|| format!("the header ends before {at}"))?;
    Ok(u32::from_le_bytes(b.try_into().expect("four bytes")))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64> {
    let b = bytes.get(at..at + 8).with_context(|| format!("the header ends before {at}"))?;
    Ok(u64::from_le_bytes(b.try_into().expect("eight bytes")))
}

fn section(bytes: &[u8], at: usize, len: usize, what: &str) -> Result<Vec<u8>> {
    let end = at.checked_add(len).with_context(|| format!("{what} runs past the end of the image"))?;
    bytes.get(at..end).map(<[u8]>::to_vec).ok_or_else(|| anyhow::anyhow!("{what} runs past the end of the image"))
}

/// A header field of `len` bytes, ending where its NUL does.
fn field(bytes: &[u8], at: usize, len: usize, what: &str) -> Result<Vec<u8>> {
    let field = section(bytes, at, len, what)?;
    Ok(field.split(|b| *b == 0).next().unwrap_or_default().to_vec())
}

fn aligned(at: usize, page: usize) -> usize {
    at.div_ceil(page) * page
}

impl VendorBoot {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        VendorBoot::parse(&bytes).with_context(|| format!("read {}", path.display()))
    }

    /// The image `bytes` hold, as a partition holds it: the image, its
    /// vbmeta image and the footer after it are left alone.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(is_vendor_boot(bytes), "it does not start with {}", String::from_utf8_lossy(MAGIC));
        let header_version = u32_at(bytes, 8)?;
        ensure!((3..=4).contains(&header_version), "header version {header_version} is not one a vendor_boot image has");
        let page_size = u32_at(bytes, 12)?;
        ensure!(page_size.is_power_of_two(), "a page of {page_size} bytes is no page");
        let page = page_size as usize;
        let kernel_addr = u32_at(bytes, 16)?;
        let ramdisk_addr = u32_at(bytes, 20)?;
        let ramdisk_size = u32_at(bytes, 24)? as usize;
        let cmdline = field(bytes, 28, CMDLINE_LEN, "the command line")?;
        let tags_addr = u32_at(bytes, 28 + CMDLINE_LEN)?;
        let name = field(bytes, 28 + CMDLINE_LEN + 4, NAME_LEN, "the name")?;
        let header_size = u32_at(bytes, 2096)?;
        ensure!(
            header_size as usize == if header_version == 4 { 2128 } else { 2112 },
            "a version {header_version} header of {header_size} bytes"
        );
        let dtb_size = u32_at(bytes, 2100)? as usize;
        let dtb_addr = u64_at(bytes, 2104)?;

        let ramdisk_at = page;
        let dtb_at = aligned(ramdisk_at + ramdisk_size, page);
        let dtb = section(bytes, dtb_at, dtb_size, "the device tree")?;

        if header_version == 3 {
            let data = section(bytes, ramdisk_at, ramdisk_size, "the ramdisk")?;
            return Ok(VendorBoot {
                header_version,
                page_size,
                kernel_addr,
                ramdisk_addr,
                cmdline,
                tags_addr,
                name,
                dtb_addr,
                fragments: vec![Fragment { name: String::new(), kind: 1, board_id: [0; BOARD_ID], data }],
                dtb,
                bootconfig: Vec::new(),
            });
        }

        let table_size = u32_at(bytes, 2112)? as usize;
        let table_num = u32_at(bytes, 2116)? as usize;
        let entry_size = u32_at(bytes, 2120)? as usize;
        let bootconfig_size = u32_at(bytes, 2124)? as usize;
        ensure!(entry_size == ENTRY_LEN, "table entries of {entry_size} bytes are not the {ENTRY_LEN} there are");
        ensure!(table_size >= table_num * entry_size, "a table of {table_size} bytes holds no {table_num} entries");

        let table_at = aligned(dtb_at + dtb_size, page);
        let table = section(bytes, table_at, table_size, "the ramdisk table")?;
        let mut fragments = Vec::with_capacity(table_num);
        for i in 0..table_num {
            let e = &table[i * entry_size..(i + 1) * entry_size];
            let size = u32::from_le_bytes(e[0..4].try_into().expect("four bytes")) as usize;
            let offset = u32::from_le_bytes(e[4..8].try_into().expect("four bytes")) as usize;
            let kind = u32::from_le_bytes(e[8..12].try_into().expect("four bytes"));
            let name = e[12..12 + ENTRY_NAME_LEN].split(|b| *b == 0).next().unwrap_or_default();
            let name = String::from_utf8_lossy(name).into_owned();
            let mut board_id = [0u32; BOARD_ID];
            for (j, id) in board_id.iter_mut().enumerate() {
                let at = 12 + ENTRY_NAME_LEN + j * 4;
                *id = u32::from_le_bytes(e[at..at + 4].try_into().expect("four bytes"));
            }
            let data = section(bytes, ramdisk_at + offset, size, &format!("ramdisk {name:?}"))?;
            fragments.push(Fragment { name, kind, board_id, data });
        }

        let bootconfig_at = aligned(table_at + table_size, page);
        let bootconfig = section(bytes, bootconfig_at, bootconfig_size, "the boot config")?;

        Ok(VendorBoot { header_version, page_size, kernel_addr, ramdisk_addr, cmdline, tags_addr, name, dtb_addr, fragments, dtb, bootconfig })
    }

    /// The image with sizes and offsets taken from its parts: a version 4
    /// header keeps its version and table, a version 3 one its single
    /// ramdisk.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let page = self.page_size as usize;
        let ramdisk_size: usize = self.fragments.iter().map(|f| f.data.len()).sum();
        let header_len = if self.header_version == 4 { 2128 } else { 2112 };
        let mut out = vec![0u8; page];
        out[..8].copy_from_slice(MAGIC);
        out[8..12].copy_from_slice(&self.header_version.to_le_bytes());
        out[12..16].copy_from_slice(&self.page_size.to_le_bytes());
        out[16..20].copy_from_slice(&self.kernel_addr.to_le_bytes());
        out[20..24].copy_from_slice(&self.ramdisk_addr.to_le_bytes());
        let ramdisk_len = u32::try_from(ramdisk_size).context("the ramdisk is too large")?;
        out[24..28].copy_from_slice(&ramdisk_len.to_le_bytes());
        out[28..28 + CMDLINE_LEN].copy_from_slice(&padded(&self.cmdline, CMDLINE_LEN, "the command line")?);
        out[28 + CMDLINE_LEN..32 + CMDLINE_LEN].copy_from_slice(&self.tags_addr.to_le_bytes());
        out[32 + CMDLINE_LEN..32 + CMDLINE_LEN + NAME_LEN].copy_from_slice(&padded(&self.name, NAME_LEN, "the name")?);
        out[2096..2100].copy_from_slice(&(header_len as u32).to_le_bytes());
        let dtb_len = u32::try_from(self.dtb.len()).context("the device tree is too large")?;
        out[2100..2104].copy_from_slice(&dtb_len.to_le_bytes());
        out[2104..2112].copy_from_slice(&self.dtb_addr.to_le_bytes());
        if self.header_version == 4 {
            let entry_len = u32::try_from(ENTRY_LEN).context("table entry")?;
            let table_len = u32::try_from(self.fragments.len() * ENTRY_LEN).context("the table is too large")?;
            out[2112..2116].copy_from_slice(&table_len.to_le_bytes());
            out[2116..2120].copy_from_slice(&(self.fragments.len() as u32).to_le_bytes());
            out[2120..2124].copy_from_slice(&entry_len.to_le_bytes());
            let bootconfig_len = u32::try_from(self.bootconfig.len()).context("the boot config is too large")?;
            out[2124..2128].copy_from_slice(&bootconfig_len.to_le_bytes());
        }
        out.resize(page, 0);

        let mut offsets = Vec::with_capacity(self.fragments.len());
        for f in &self.fragments {
            offsets.push(u32::try_from(out.len() - page).context("a ramdisk offset")?);
            out.extend_from_slice(&f.data);
        }
        out.resize(aligned(out.len(), page), 0);
        out.extend_from_slice(&self.dtb);
        out.resize(aligned(out.len(), page), 0);

        if self.header_version == 4 {
            for (f, offset) in self.fragments.iter().zip(&offsets) {
                let at = out.len();
                out.resize(at + ENTRY_LEN, 0);
                let e = &mut out[at..];
                e[0..4].copy_from_slice(&(f.data.len() as u32).to_le_bytes());
                e[4..8].copy_from_slice(&offset.to_le_bytes());
                e[8..12].copy_from_slice(&f.kind.to_le_bytes());
                let name = f.name.as_bytes();
                ensure!(name.len() <= ENTRY_NAME_LEN, "a ramdisk name of {} bytes", name.len());
                e[12..12 + name.len()].copy_from_slice(name);
                for (j, id) in f.board_id.iter().enumerate() {
                    let at = 12 + ENTRY_NAME_LEN + j * 4;
                    e[at..at + 4].copy_from_slice(&id.to_le_bytes());
                }
            }
            out.resize(aligned(out.len(), page), 0);
            out.extend_from_slice(&self.bootconfig);
            out.resize(aligned(out.len(), page), 0);
        }
        Ok(out)
    }
}

/// `value` in a field of `len` bytes, zero-filled after it.
fn padded(value: &[u8], len: usize, what: &str) -> Result<Vec<u8>> {
    if value.len() > len {
        bail!("{what} takes more than {len} bytes");
    }
    let mut out = vec![0u8; len];
    out[..value.len()].copy_from_slice(value);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(header_version: u32) -> VendorBoot {
        let mut board_id = [0u32; BOARD_ID];
        board_id[0] = 0x1234;
        VendorBoot {
            header_version,
            page_size: 4096,
            kernel_addr: 0x8000_0000,
            ramdisk_addr: 0x66f0_0000,
            cmdline: b"bootopt=64S3,32N2,64N2".to_vec(),
            tags_addr: 0x8000_0100,
            name: b"pickle".to_vec(),
            dtb_addr: 0x47c8_0000,
            fragments: vec![
                Fragment { name: "platform".into(), kind: 1, board_id: [0; BOARD_ID], data: vec![b'P'; 5000] },
                Fragment { name: "recovery".into(), kind: 2, board_id, data: vec![b'R'; 3000] },
            ],
            dtb: vec![b'D'; 700],
            bootconfig: b"androidboot.foo=1\n".to_vec(),
        }
    }

    #[test]
    fn a_version_4_image_keeps_its_fragments_their_table_and_boot_config() {
        let packed = image(4).to_bytes().unwrap();
        let back = VendorBoot::parse(&packed).unwrap();
        assert_eq!((back.header_version, back.page_size, back.dtb_addr), (4, 4096, 0x47c8_0000));
        assert_eq!(back.cmdline, image(4).cmdline);
        assert_eq!(back.fragments.len(), 2);
        assert_eq!(back.fragments[1].name, "recovery");
        assert_eq!(back.fragments[1].kind, 2);
        assert_eq!(back.fragments[1].board_id[0], 0x1234, "the board IDs are kept");
        assert!(back.fragments[0].data.iter().all(|b| *b == b'P'));
        assert_eq!(back.dtb, vec![b'D'; 700]);
        assert_eq!(back.bootconfig, b"androidboot.foo=1\n");
        assert_eq!(packed.len() % 4096, 0, "the image ends on a page");
    }

    #[test]
    fn a_version_3_image_carries_one_ramdisk_and_no_table() {
        let mut img = image(3);
        img.fragments.truncate(1);
        let packed = img.to_bytes().unwrap();
        assert_eq!(packed.len(), 4096 + 8192 + 4096, "header, one page of ramdisk and the device tree");
        let back = VendorBoot::parse(&packed).unwrap();
        assert_eq!(back.fragments.len(), 1);
        assert_eq!(back.fragments[0].data.len(), 5000);
        assert!(back.bootconfig.is_empty());
    }

    #[test]
    fn bytes_that_are_no_vendor_boot_image_are_refused() {
        assert!(VendorBoot::parse(b"ANDROID!....................").is_err());
        assert!(!is_vendor_boot(b"ANDROID!"));
        assert!(is_vendor_boot(b"VNDRBOOT"));
    }
}
