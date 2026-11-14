//! Android boot images with header version 3 or 4, the vendor_boot images
//! whose ramdisk holds a recovery on phones that keep it out of a partition
//! of its own, and the newc archives the ramdisks are: read, changed and
//! written again, by the image packer on a computer and by the recovery on
//! the phone.

mod image;
mod ramdisk;
mod vendorboot;

pub use image::BootImage;
pub use ramdisk::{lz4_legacy, Entry, Ramdisk};
pub use vendorboot::{is_vendor_boot, Fragment, VendorBoot};
