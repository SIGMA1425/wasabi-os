#![no_std]
#![feature(offset_of)]
#![no_main]

use core::fmt::Write;
use core::panic::PanicInfo;
use core::time::Duration;

use core::writeln;
use wasabi::executor::Executor;
use wasabi::executor::Task;
use wasabi::executor::TimeoutFuture;
use wasabi::graphics::draw_test_pattern;
use wasabi::graphics::fill_rect;
use wasabi::graphics::Bitmap;

use wasabi::info;
use wasabi::init::init_allocator;
use wasabi::init::init_hpet;
use wasabi::init::init_paging;
use wasabi::qemu::exit_qemu;
use wasabi::qemu::QemuExitCode;

use wasabi::uefi::init_vram;
use wasabi::uefi::locate_loaded_image_protocol;
use wasabi::uefi::EfiHandle;
use wasabi::uefi::EfiSystemTable;
use wasabi::uefi::VramTextWriter;

use wasabi::warn;

use wasabi::error;
use wasabi::init::init_basic_runtime;
use wasabi::print::hexdump;
use wasabi::println;

use wasabi::x86::flush_tlb;
use wasabi::x86::init_exceptions;
use wasabi::x86::read_cr3;
use wasabi::x86::trigger_debug_interrupt;
use wasabi::x86::PageAttr;

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

    let vw = vram.width();
    let vh = vram.height();
    fill_rect(&mut vram, 0x000000, 0, 0, vw, vh).expect("fill_rect failed");

    draw_test_pattern(&mut vram);

    let mut w = VramTextWriter::new(&mut vram);
    let memory_map = init_basic_runtime(image_handle, efi_system_table);
    init_allocator(&memory_map);
    writeln!(w, "Hello, Non-UEFI world!").unwrap();

    let cr3 = wasabi::x86::read_cr3();
    println!("cr3 = {cr3:#p}");
    // hexdump(unsafe{ &*cr3});
    let t = Some(unsafe { &*cr3 });
    println!("{t:?}");
    let t = t.and_then(|t| t.next_level(0));
    println!("{t:?}");
    let t = t.and_then(|t| t.next_level(0));
    println!("{t:?}");
    let t = t.and_then(|t| t.next_level(0));
    println!("{t:?}");

    // 例外の初期化
    let (_gdt, _idt) = init_exceptions();
    info!("Exception initialized!");
    // INT3命令を実行する
    // INT3命令実行時にsrc/x86.rs内で定義した例外ハンドラに処理を移す
    // 例外ハンドラ実行後にIRET命令を実行して例外ハンドラの処理を終了
    trigger_debug_interrupt();
    info!("Exception continued");
    init_paging(&memory_map);
    info!("Now we are using our own page tables!");

    let page_table = read_cr3();
    unsafe {
        (*page_table)
            .create_mapping(0, 4096, 0, PageAttr::NotPresent)
            .expect("Failed to unmap page 0");
    }
    flush_tlb();

    init_hpet(acpi);
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

    let mut executor = Executor::new();
    executor.enqueue(task1);
    executor.enqueue(task2);
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
