#![no_std]
#![feature(offset_of)]
#![no_main]

use core::panic::PanicInfo;
use core::time::Duration;

use wasabi::executor::Executor;
use wasabi::executor::Task;
use wasabi::executor::TimeoutFuture;

use wasabi::info;
use wasabi::init::init_allocator;
use wasabi::init::init_display;
use wasabi::init::init_hpet;
use wasabi::init::init_paging;
use wasabi::init::init_pci;
use wasabi::qemu::exit_qemu;
use wasabi::qemu::QemuExitCode;

use wasabi::serial::SerialPort;
use wasabi::uefi::init_vram;
use wasabi::uefi::locate_loaded_image_protocol;
use wasabi::uefi::EfiHandle;
use wasabi::uefi::EfiSystemTable;

use wasabi::warn;

use wasabi::error;
use wasabi::init::init_basic_runtime;
use wasabi::print::hexdump;
use wasabi::print::set_global_vram;
use wasabi::println;

use wasabi::x86::init_exceptions;

use wasabi::hpet::global_timestamp;

#[no_mangle]
fn efi_main(image_handle: EfiHandle, efi_system_table: &EfiSystemTable) {
    println!("Booting WasabiOS...");
    println!("image_handle: {:#018X}", image_handle);
    println!("efi_system_table: {:#p}", efi_system_table);
    let loaded_image_protocol = locate_loaded_image_protocol(image_handle, efi_system_table)
        .expect("Failed to get LoadedImageProtocol");
    println!("image_base: {:#018X}", loaded_image_protocol.image_base);
    println!("image_size: {:#018X}", loaded_image_protocol.image_size);
    info!("info");
    warn!("warn");
    error!("error");
    hexdump(efi_system_table);
    let mut vram = init_vram(efi_system_table).expect("init_vram failed");
    let acpi = efi_system_table.acpi_table().expect("ACPI table not found");
    info!("{acpi:#p}");
    hexdump(acpi);

    init_display(&mut vram);

    set_global_vram(vram);
    let memory_map = init_basic_runtime(image_handle, efi_system_table);
    init_allocator(&memory_map);
    info!("Hello, Non-UEFI world!\nThis is test");

    // 例外の初期化
    let (_gdt, _idt) = init_exceptions();

    init_paging(&memory_map);

    init_hpet(acpi);
    init_pci(acpi);
    let t0 = global_timestamp();

    let task1 = Task::new(async move {
        for i in 100..=103 {
            info!("{i} hpet.main_counter = {:?}", global_timestamp() - t0);
            TimeoutFuture::new(Duration::from_secs(1)).await
        }
        Ok(())
    });

    let task2 = Task::new(async move {
        for i in 200..=203 {
            info!("{i} hpet.main_counter = {:?}", global_timestamp() - t0);
            TimeoutFuture::new(Duration::from_secs(2)).await
        }
        Ok(())
    });
    let serial_task = Task::new(async {
        let sp = SerialPort::default();
        if let Err(e) = sp.loopback_test() {
            error!("{e:?}");
            return Err("serial: loopback test failed");
        }
        info!("Started to monitor serial port");
        loop {
            if let Some(v) = sp.try_read() {
                let c = char::from_u32(v as u32);
                info!("serial input: {v:#04X} = {c:?}");
            }
            TimeoutFuture::new(Duration::from_millis(20)).await;
        }
    });

    let mut executor = Executor::new();
    executor.enqueue(task1);
    executor.enqueue(task2);
    executor.enqueue(serial_task);
    Executor::run(executor)
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // loop {
    //     hlt()
    // }

    exit_qemu(QemuExitCode::Fail)
}

// pub fn hlt(){
//     unsafe {asm!("hlt")}
// }
