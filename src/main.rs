//! Ampon on Anchor in Rust
#![no_std]
#![no_main]
#![allow(dead_code, non_camel_case_types, non_upper_case_globals)]

use panic_halt as _;

use core::{arch::asm, ptr};
use embedded_hal as ehal;
use hal::pac;
pub use stm32f0xx_hal as hal;

use crate::ehal::spi::MODE_3;
use crate::hal::prelude::*;
use crate::hal::spi::{EightBit, Spi};

use cortex_m::interrupt::free;
use cortex_m_rt::entry;

use hal::usb::Peripheral;
use hal::{gpio::gpioa::*, gpio::*, stm32, stm32::SPI1};
use pac::CorePeripherals;

use anchor::*;

mod adxl345;
mod clock;
mod commands;
mod usb;

const BOOTLOADER_FLAG_ADDR: u32 = 0x2000_15fc; // 0 and max get clobbered at init
const BOOTLOADER_FLAG_MAGIC: u32 = 0xf026_69ef;
const BOOTLOADER_ST_ADDR: u32 = 0x1fff_c400;

const LED_TOGGLE_TICKS: u32 = CLOCK_FREQ / 8;

type AmponAdxl = adxl345::Adxl<
    Spi<SPI1, PA5<Alternate<AF0>>, PA6<Alternate<AF0>>, PA7<Alternate<AF0>>, EightBit>,
    PA4<Output<PushPull>>,
>;

pub struct State {
    config_crc: Option<u32>,
    adxl: AmponAdxl,
}

impl State {
    fn new(adxl: AmponAdxl) -> Self {
        State {
            config_crc: None,
            adxl,
        }
    }

    fn clock(&self) -> &clock::Clock {
        &ampon_global().clock
    }
}

pub struct Ampon {
    core: CorePeripherals,
    pin_led: PA14<Output<PushPull>>,
    led_tick_time: Option<clock::InstantShort>,
    state: State,
}

pub struct AmponGlobal {
    usb: usb::AmponUsb,
    clock: clock::Clock,
}

struct AmponParts {
    ampon: Ampon,
    global: AmponGlobal,
}

impl Ampon {
    fn init() -> AmponParts {
        let mut dp = stm32::Peripherals::take().unwrap();
        let core = CorePeripherals::take().unwrap();

        dp.RCC.apb1enr.modify(|_, w| w.tim2en().set_bit());
        dp.RCC.apb1rstr.modify(|_, w| w.tim2rst().set_bit());
        dp.RCC.apb1rstr.modify(|_, w| w.tim2rst().clear_bit());
        dp.TIM2.cr1.modify(|_, w| w.cen().set_bit());

        hal::usb::remap_pins(&mut dp.RCC, &mut dp.SYSCFG);

        //dp.RCC.apb1enr.modify(|_, w| w.usben().set_bit());
        //dp.USB.cntr.modify(|_, w| w.fres().set_bit());
        //dp.USB.cntr.modify(|_, w| w.fres().clear_bit());

        let mut rcc = dp
            .RCC
            .configure()
            .hsi48()
            .enable_crs(dp.CRS)
            .sysclk(48.mhz())
            .pclk(48.mhz())
            .freeze(&mut dp.FLASH);

        // Configure the on-board LED (LD3, green)
        let gpioa = dp.GPIOA.split(&mut rcc);
        let gpiob = dp.GPIOB.split(&mut rcc);

        // bootloader magic
        unsafe {
            if ptr::read(BOOTLOADER_FLAG_ADDR as *const u32) == !BOOTLOADER_FLAG_MAGIC {
                ptr::write(BOOTLOADER_FLAG_ADDR as *mut u32, 0);
                asm!("nop");
                cortex_m::peripheral::SCB::sys_reset();
            }
            if ptr::read(BOOTLOADER_FLAG_ADDR as *const u32) == BOOTLOADER_FLAG_MAGIC {
                ptr::write(BOOTLOADER_FLAG_ADDR as *mut u32, !BOOTLOADER_FLAG_MAGIC);
                let mut pin_bl =
                    cortex_m::interrupt::free(|cs| gpiob.pb8.into_push_pull_output(cs));
                pin_bl.set_high().ok(); // f042 has a bug (feature?) where it won't run bl if pin isn't physically asserted
                let initial_sp = ptr::read(BOOTLOADER_ST_ADDR as *const u32);
                let start_addr = ptr::read((BOOTLOADER_ST_ADDR + 4) as *const u32);
                asm!("mov sp, {0}\nbx {1}", in(reg) initial_sp, in(reg) start_addr);
            }
        }

        let mut pin_led = cortex_m::interrupt::free(|cs| gpioa.pa14.into_push_pull_output(cs));
        pin_led.set_high().ok(); // Turn on

        let mcu_clock = clock::Clock::init(dp.TIM2);

        let usb_peripheral = Peripheral {
            usb: dp.USB,
            pin_dm: gpioa.pa11,
            pin_dp: gpioa.pa12,
        };
        let usb = usb::AmponUsb::init(usb_peripheral);

        let cs = cortex_m::interrupt::free(|cs| gpioa.pa4.into_push_pull_output(cs));

        let (sck, miso, mosi) = cortex_m::interrupt::free(move |cs| {
            (
                gpioa.pa5.into_alternate_af0(cs),
                gpioa.pa6.into_alternate_af0(cs),
                gpioa.pa7.into_alternate_af0(cs),
            )
        });

        let spi = Spi::spi1(dp.SPI1, (sck, miso, mosi), MODE_3, 8.mhz(), &mut rcc);
        let adxl = AmponAdxl::init(spi, cs);

        AmponParts {
            ampon: Ampon {
                core,
                pin_led,
                led_tick_time: None,
                state: State::new(adxl),
            },
            global: AmponGlobal {
                usb,
                clock: mcu_clock,
            },
        }
    }

    fn run_usb<const BUF_SIZE: usize>(&mut self, receive_buffer: &mut FifoBuffer<BUF_SIZE>) {
        // Pump USB read side
        ampon_global().usb.read_into(receive_buffer);
        let recv_data = receive_buffer.data();
        if !recv_data.is_empty() {
            let mut wrap = SliceInputBuffer::new(recv_data);
            KLIPPER_TRANSPORT.receive(&mut wrap, &mut self.state);
            let consumed = recv_data.len() - wrap.available();
            if consumed > 0 {
                receive_buffer.pop(consumed);
            }
        }

        // Pump USB write side
        free(|cs| {
            let mut txbuf = usb::USB_TX_BUFFER.borrow(cs).borrow_mut();
            ampon_global().usb.write_from(cs, &mut txbuf);
        });

        if ampon_global().usb.data_rate() == 1200 {
            unsafe {
                ptr::write(BOOTLOADER_FLAG_ADDR as *mut u32, BOOTLOADER_FLAG_MAGIC);
                asm!("nop"); // write needs time to hit ram
            }
            cortex_m::peripheral::SCB::sys_reset();
        }
    }

    fn run_adxl(&mut self, t: clock::InstantShort) {
        self.state.adxl.run(t);
    }

    fn run_led(&mut self, t: clock::InstantShort) {
        if !self.state.adxl.detected() {
            if let Some(tick) = self.led_tick_time {
                if t.after(tick) {
                    self.pin_led.toggle().ok();
                    self.led_tick_time = Some(tick + LED_TOGGLE_TICKS);
                }
            } else {
                self.led_tick_time = Some(t + LED_TOGGLE_TICKS);
            }
        }
    }
}

impl AmponParts {
    fn run_forever(mut self) -> ! {
        unsafe {
            AMPON_GLOBAL = Some(self.global);
        }

        // We keep the receive buffer here, outside of the State struct.
        // This allows us to later pass the State struct as our context to our commands handlers.
        // If the receive buffer was part of the State struct, we'd have multiple mutable borrows.
        let mut receive_buffer = FifoBuffer::<{ usb::USB_MAX_PACKET_SIZE * 2 }>::new();

        loop {
            ampon_global().usb.poll();
            self.ampon.run_usb(&mut receive_buffer);
            self.ampon.run_adxl(ampon_global().clock.low());
            self.ampon.run_led(ampon_global().clock.low());
        }
    }
}

static mut AMPON_GLOBAL: Option<AmponGlobal> = None;

pub fn ampon_global() -> &'static AmponGlobal {
    unsafe { AMPON_GLOBAL.as_ref().unwrap() }
}

#[entry]
fn main() -> ! {
    Ampon::init().run_forever();
}

klipper_config_generate!(
    transport = crate::usb::TRANSPORT_OUTPUT: crate::usb::BufferTransportOutput,
    context = &'ctx mut crate::State,
);

#[klipper_constant]
const CLOCK_FREQ: u32 = 48_000_000;

#[klipper_constant]
const MCU: &str = "ampon";

#[klipper_constant]
const STATS_SUMSQ_BASE: u32 = 256;

klipper_enumeration!(
    enum spi_bus {
        spi1,
    }
);

klipper_enumeration!(
    enum pin {
        CS,
    }
);

#[klipper_constant]
const BUS_PINS_spi1: &str = "PA5,PA6,PA7";
