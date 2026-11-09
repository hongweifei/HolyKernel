//! USB HID 服务：使用 `crab-usb` 的 xHCI 后端枚举 boot keyboard/mouse，
//! 把按键、指针、按钮与滚轮事件转发给显示服务。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_device::DeviceGrant;
#[cfg(target_os = "none")]
use hk_driver_host::{init_dma, init_log, init_mmio, register_mmio};
#[cfg(target_os = "none")]
use hk_driver_usb::{block_on, crab_usb, kernel};
#[cfg(target_os = "none")]
use hk_proto::input::{
    INPUT_BUTTON, INPUT_KEY, INPUT_POINTER, INPUT_WHEEL, pack_pointer, pack_wheel,
};
#[cfg(target_os = "none")]
use hk_server_api::device::{DEVICE_CLASS_USB, DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, device_info, device_map, exit, irq_bind, recv_handle, self_notification, send,
    service_endpoint, wait_notify, write, write_u64,
};

#[cfg(target_os = "none")]
use crab_usb::device::DeviceInfo;
#[cfg(target_os = "none")]
use crab_usb::usb_if::descriptor::EndpointType;
#[cfg(target_os = "none")]
use crab_usb::usb_if::endpoint::{RequestId, TransferRequest};
#[cfg(target_os = "none")]
use crab_usb::usb_if::err::TransferError;
#[cfg(target_os = "none")]
use crab_usb::usb_if::host::ControlSetup;
#[cfg(target_os = "none")]
use crab_usb::usb_if::transfer::{Direction, Recipient, Request, RequestType};
#[cfg(target_os = "none")]
use crab_usb::{Device, EndpointHandle, InterfaceSession, USBHost};

#[cfg(any(target_os = "none", test))]
mod hid;

#[cfg(target_os = "none")]
use hid::{Event, KeyboardState, MouseState};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 512 * 1024;

#[cfg(target_os = "none")]
const MAX_HID: usize = 8;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

#[cfg(target_os = "none")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Keyboard,
    Mouse,
}

#[cfg(target_os = "none")]
struct HidDevice {
    kind: Kind,
    _device: Device,
    _session: InterfaceSession,
    endpoint: EndpointHandle,
    buffer: [u8; 8],
    pending: Option<RequestId>,
    keyboard: KeyboardState,
    mouse: MouseState,
}

#[cfg(target_os = "none")]
impl HidDevice {
    fn submit(&mut self) {
        let request = TransferRequest::interrupt_in(&mut self.buffer);
        self.pending = self.endpoint.submit(request).ok();
    }

    fn poll(&mut self, out: &mut Vec<Event>) {
        if self.pending.is_none() {
            self.submit();
        }
        let Some(id) = self.pending else {
            return;
        };
        match self.endpoint.reclaim(id) {
            Ok(Some(completion)) => {
                self.pending = None;
                if completion.status == crab_usb::usb_if::endpoint::TransferStatus::Completed {
                    let report = &self.buffer[..completion.actual_length.min(self.buffer.len())];
                    match self.kind {
                        Kind::Keyboard => self.keyboard.report(report, out),
                        Kind::Mouse => self.mouse.report(report, out),
                    }
                }
                self.submit();
            }
            Ok(None) => {}
            Err(TransferError::Disconnected) => {
                self.pending = None;
            }
            Err(_) => {
                self.pending = None;
                self.submit();
            }
        }
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-usb: start\n");
    let manager = service_endpoint("hk-server-device");
    if manager == 0 {
        write(b"hk-server-usb: no device manager\n");
        exit(0)
    }
    let Some(grant) = request_grant(endpoint, manager) else {
        write(b"hk-server-usb: no USB controller\n");
        exit(0)
    };
    let mut bytes = [0_u8; hk_device::DEVICE_GRANT_BYTES];
    let length = device_info(grant, &mut bytes) as usize;
    let Some(descriptor) = (length != u64::MAX as usize)
        .then(|| DeviceGrant::decode(&bytes[..length]))
        .flatten()
    else {
        write(b"hk-server-usb: invalid grant\n");
        exit(2)
    };
    let Some((index, range)) = descriptor
        .mmio
        .iter()
        .enumerate()
        .find_map(|(index, range)| range.map(|range| (index, range)))
    else {
        write(b"hk-server-usb: no MMIO BAR\n");
        exit(2)
    };
    let Some(virt) = device_map(grant, index as u64) else {
        write(b"hk-server-usb: MMIO map failed\n");
        exit(2)
    };
    let Ok(range_len) = usize::try_from(range.len) else {
        write(b"hk-server-usb: BAR too large\n");
        exit(2)
    };

    init_log();
    init_mmio();
    register_mmio(range.base, virt, range_len);
    let Some(mmio) = hk_driver_host::mmio::ioremap(range.base, range_len) else {
        write(b"hk-server-usb: ioremap failed\n");
        exit(3)
    };
    let dma = init_dma(grant);
    let Some(mut host) = USBHost::new_xhci(mmio.as_nonnull_ptr(), dma, kernel()).ok() else {
        write(b"hk-server-usb: xHCI init failed\n");
        exit(3)
    };
    let events = host.create_event_handler();

    let notification = self_notification();
    if irq_bind(grant, notification, 1) != 0 {
        write(b"hk-server-usb: irq bind failed\n");
        exit(3)
    }
    if block_on(host.init(), notification, &events).is_err() {
        write(b"hk-server-usb: controller start failed\n");
        exit(3)
    }

    let Some(changes) = block_on(host.probe_devices(), notification, &events).ok() else {
        write(b"hk-server-usb: probe failed\n");
        exit(3)
    };
    let mut devices = Vec::new();
    for device in changes.connected {
        let Some(info) = device.into_device_info() else {
            continue;
        };
        if devices.len() >= MAX_HID {
            break;
        }
        if let Some(hid) = open_hid(&mut host, &info, notification, &events) {
            devices.push(hid);
        }
    }

    if devices.is_empty() {
        write(b"hk-server-usb: no HID device\n");
        exit(0)
    }
    write(b"hk-server-usb: devices ");
    write_u64(devices.len() as u64);
    write(b"\n");

    loop {
        let mut out = Vec::new();
        for device in &mut devices {
            device.poll(&mut out);
        }
        for event in out {
            forward(event);
        }
        events.handle_event();
        let _ = wait_notify(notification, 1);
    }
}

#[cfg(target_os = "none")]
fn open_hid(
    host: &mut USBHost,
    info: &DeviceInfo,
    notification: u64,
    events: &crab_usb::EventHandler,
) -> Option<HidDevice> {
    for interface in info.interface_descriptors() {
        let Some(kind) = kind(interface) else {
            continue;
        };
        let Some(endpoint) = interface.endpoints.iter().find(|endpoint| {
            endpoint.direction == Direction::In && endpoint.transfer_type == EndpointType::Interrupt
        }) else {
            continue;
        };
        let mut device = block_on(host.open_device(info), notification, events).ok()?;
        if set_boot_protocol(
            &mut device,
            u16::from(interface.interface_number),
            notification,
            events,
        )
        .is_err()
        {
            continue;
        }
        let session = block_on(
            device.claim_interface(interface.interface_number, 0),
            notification,
            events,
        )
        .ok()?;
        let endpoint = session.endpoint(endpoint.address).ok()?;
        let mut hid = HidDevice {
            kind,
            _device: device,
            _session: session,
            endpoint,
            buffer: [0; 8],
            pending: None,
            keyboard: KeyboardState::default(),
            mouse: MouseState::default(),
        };
        hid.submit();
        return Some(hid);
    }
    None
}

#[cfg(target_os = "none")]
fn kind(interface: &crab_usb::usb_if::descriptor::InterfaceDescriptor) -> Option<Kind> {
    if interface.class != 0x03 {
        return None;
    }
    match interface.protocol {
        1 => Some(Kind::Keyboard),
        2 => Some(Kind::Mouse),
        _ => None,
    }
}

#[cfg(target_os = "none")]
fn set_boot_protocol(
    device: &mut Device,
    interface: u16,
    notification: u64,
    events: &crab_usb::EventHandler,
) -> Result<(), TransferError> {
    let setup = ControlSetup {
        request_type: RequestType::Class,
        recipient: Recipient::Interface,
        request: Request::Other(0x0B),
        value: 0,
        index: interface,
    };
    block_on(device.control_out(setup, &[]), notification, events).map(|_| ())
}

#[cfg(target_os = "none")]
fn forward(event: Event) {
    let display = service_endpoint("hk-server-display");
    if display == 0 {
        return;
    }
    match event {
        Event::Key(byte) => {
            let _ = send(display, INPUT_KEY, u64::from(byte));
        }
        Event::Pointer(x, y) => {
            let _ = send(display, INPUT_POINTER, pack_pointer(x, y));
        }
        Event::Button(pressed) => {
            let _ = send(display, INPUT_BUTTON, u64::from(pressed));
        }
        Event::Wheel(delta) => {
            let _ = send(display, INPUT_WHEEL, pack_wheel(delta));
        }
    }
}

#[cfg(target_os = "none")]
fn request_grant(endpoint: u64, manager: u64) -> Option<u64> {
    let _ = send(manager, DEVICE_REQUEST, DEVICE_CLASS_USB);
    loop {
        let (label, _word, _sender, handle) = recv_handle(endpoint);
        if label == DEVICE_GRANT && handle != 0 {
            return Some(handle);
        }
        if label == DEVICE_NONE {
            return None;
        }
        if handle != 0 {
            let _ = close_handle(handle);
        }
    }
}

hk_server_api::server_boilerplate!();
