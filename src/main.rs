#![no_std]
#![no_main]

use core::cell::RefCell;
use core::fmt::Arguments;
use core::sync::atomic::{AtomicBool, Ordering};

use cyw43::JoinOptions;
use cyw43_driver::{net_task, setup_cyw43};
use defmt::*;
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice;
use embassy_executor::Spawner;
use embassy_net::{Config, StackResources};
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio;
use embassy_rp::gpio::Input;
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi;
use embassy_rp::spi::Spi;
use embassy_sync::blocking_mutex;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::mutex::Mutex;
use embassy_time::Delay;
use embassy_time::{Duration, Timer};
use embedded_graphics::primitives::PrimitiveStyleBuilder;
use embedded_graphics::text::Text;
use embedded_graphics::{
    mono_font::{ascii::*, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use embedded_text::{
    alignment::HorizontalAlignment,
    style::{HeightMode, TextBoxStyleBuilder},
    TextBox,
};
use gpio::{Level, Output, Pull};
use heapless::String;
use rand::RngCore;
use static_cell::StaticCell;
use uc8151::asynch::Uc8151;
use uc8151::LUT;
use uc8151::WIDTH;
use {defmt_rtt as _, panic_probe as _};

mod cyw43_driver;
mod env;

pub static DISPLAY_HAS_CHANGED: AtomicBool = AtomicBool::new(false);
pub static IP: blocking_mutex::Mutex<CriticalSectionRawMutex, RefCell<String<20>>> =
    blocking_mutex::Mutex::new(RefCell::new(String::<20>::new()));
type Spi0Bus = Mutex<NoopRawMutex, Spi<'static, SPI0, spi::Async>>;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let (net_device, mut control) = setup_cyw43(
        p.PIO0, p.PIN_23, p.PIN_24, p.PIN_25, p.PIN_29, p.DMA_CH0, spawner,
    )
    .await;
    let miso = p.PIN_16;
    let mosi = p.PIN_19;
    let clk = p.PIN_18;

    let dc = Output::new(p.PIN_20, Level::Low);
    let cs = Output::new(p.PIN_17, Level::High);
    let busy = Input::new(p.PIN_26, Pull::Up);
    let reset = Output::new(p.PIN_21, Level::Low);

    let delay: Duration = Duration::from_secs(3);
    let spi = Spi::new(
        p.SPI0,
        clk,
        mosi,
        miso,
        p.DMA_CH1,
        p.DMA_CH2,
        spi::Config::default(),
    );
    static SPI_BUS: StaticCell<Spi0Bus> = StaticCell::new();
    let spi_bus = SPI_BUS.init(Mutex::new(spi));
    spawner.must_spawn(run_the_display(spi_bus, cs, dc, busy, reset));

    let mut rng = RoscRng;
    let wifi_ssid = env::env_value("WIFI_SSID");
    let wifi_password = env::env_value("WIFI_PASSWORD");
    //Configures the pico to use DHCP
    let config = Config::dhcpv4(Default::default());
    // Generate random seed
    let seed = rng.next_u64();

    // Init network stack
    static RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        net_device,
        config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    unwrap!(spawner.spawn(net_task(runner)));

    loop {
        match control
            .join(wifi_ssid, JoinOptions::new(wifi_password.as_bytes()))
            .await
        {
            Ok(_) => break,
            Err(err) => {
                info!("join failed with status={}", err.status);
            }
        }
    }

    // Wait for DHCP, not necessary when using static IP
    info!("waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    info!("DHCP is now up!");

    info!("waiting for link up...");
    while !stack.is_link_up() {
        Timer::after_millis(500).await;
    }
    info!("Link is up!");

    info!("waiting for stack to be up...");
    stack.wait_config_up().await;
    let ip = stack.config_v4().unwrap().address;
    let ip_string = easy_format::<20>(format_args!("{}", ip));
    IP.lock(|ip| {
        let mut ip = ip.borrow_mut();
        ip.clear();
        let _ = ip.push_str(&ip_string);
        DISPLAY_HAS_CHANGED.store(true, Ordering::Relaxed);
    });
    info!("Stack is up!");
    loop {
        Timer::after(delay).await;
    }
}

#[embassy_executor::task]
pub async fn run_the_display(
    spi_bus: &'static Spi0Bus,
    cs: Output<'static>,
    dc: Output<'static>,
    busy: Input<'static>,
    reset: Output<'static>,
) {
    let spi_device = SpiDevice::new(&spi_bus, cs);
    let mut display = Uc8151::new(spi_device, dc, busy, reset, Delay);
    info!("Resetting display");
    display.reset().await;

    // Initialise display. Using the default LUT speed setting
    let test = display.setup(LUT::Medium).await;
    if test.is_err() {
        error!("Display setup failed");
    }

    let character_style = MonoTextStyle::new(&FONT_9X18_BOLD, BinaryColor::Off);
    let textbox_style = TextBoxStyleBuilder::new()
        .height_mode(HeightMode::FitToText)
        .alignment(HorizontalAlignment::Left)
        .paragraph_spacing(6)
        .build();

    // Bounding box for our text. Fill it with the opposite color so we can read the text.
    let static_text_bounds = Rectangle::new(Point::new(10, 50), Size::new(WIDTH, 0));
    static_text_bounds
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(&mut display)
        .unwrap();

    // Crate static text
    let text = "Hello BlueSky";
    let text_box =
        TextBox::with_textbox_style(text, static_text_bounds, character_style, textbox_style);

    // Draw the text box.
    text_box.draw(&mut display).unwrap();
    let _ = display.update().await;

    loop {
        if DISPLAY_HAS_CHANGED.load(Ordering::Relaxed) {
            let ip = IP.lock(|ip| ip.borrow().clone());
            let top_box = Rectangle::new(Point::new(0, 0), Size::new(WIDTH, 24));
            top_box
                .into_styled(
                    PrimitiveStyleBuilder::default()
                        .stroke_color(BinaryColor::Off)
                        .fill_color(BinaryColor::On)
                        .stroke_width(1)
                        .build(),
                )
                .draw(&mut display)
                .unwrap();

            Text::new(ip.as_str(), Point::new(8, 16), character_style)
                .draw(&mut display)
                .unwrap();

            // Draw the counter text box.
            let _ = display.partial_update(top_box.try_into().unwrap()).await;
            DISPLAY_HAS_CHANGED.store(false, Ordering::Relaxed);
        }
        Timer::after(Duration::from_millis(100)).await;
    }
}

/// Makes it easier to format strings in a single line method
fn easy_format<const N: usize>(args: Arguments<'_>) -> String<N> {
    let mut formatted_string: String<N> = String::<N>::new();
    let result = core::fmt::write(&mut formatted_string, args);
    match result {
        Ok(_) => formatted_string,
        Err(_) => {
            error!("Error formatting the string");
            //This really should be a result return type, or panic. but going keep the ball rolling
            String::<N>::new()
        }
    }
}
