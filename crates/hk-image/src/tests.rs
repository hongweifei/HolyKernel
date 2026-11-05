use crate::{
    CAP_DEVICE, CapabilityData, DependencyData, Image, RegionData, SymbolData, TaskData, build,
};

fn sample() -> alloc::vec::Vec<u8> {
    build(
        TaskData {
            name: "hk-server-demo",
            role: 1,
            priority: 16,
            entry: 0x4000,
            stack_size: 4096,
            arena_size: 8192,
        },
        &[RegionData {
            virtual_address: 0x4000,
            data: &[1, 2, 3, 4],
            memory_size: 16,
            flags: 5,
        }],
        &[CapabilityData {
            kind: CAP_DEVICE,
            rights: 3,
            name: "device",
        }],
        &[SymbolData {
            address: 0x4010,
            name: "demo::main",
        }],
        &[DependencyData {
            name: "hk-server-echo",
            min_version: 1,
        }],
    )
}

#[test]
fn round_trips_image() {
    let bytes = sample();
    let image = Image::parse(&bytes).expect("image");
    let task = image.task().expect("task");
    assert_eq!(task.name, "hk-server-demo");
    assert_eq!(task.entry, 0x4000);
    assert_eq!(task.stack_size, 4096);
    assert!(image.has_capability(CAP_DEVICE));
    let regions: alloc::vec::Vec<_> = image.regions().collect();
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].file, &[1, 2, 3, 4]);
    let symbols: alloc::vec::Vec<_> = image.symbols().collect();
    assert_eq!(symbols[0].name, "demo::main");
    let dependencies: alloc::vec::Vec<_> = image.dependencies().collect();
    assert_eq!(dependencies[0].name, "hk-server-echo");
}

#[test]
fn rejects_bad_checksum() {
    let mut image = sample();
    let last = image.len() - 1;
    image[last] ^= 0xFF;
    assert!(Image::parse(&image).is_none());
}

#[test]
fn rejects_bad_magic() {
    assert!(Image::parse(b"not an image").is_none());
}

#[test]
fn checksum_matches_crc32c_vector() {
    assert_eq!(crate::format::crc32c(b"123456789"), 0xE306_9283);
}
