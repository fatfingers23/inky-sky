//! This example test the RP Pico W on board LED.
//!
//! It does not work with the RP Pico board.

#![no_std]
#![no_main]

use cyw43_driver::setup_cyw43;
use defmt::*;
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice;

use embassy_executor::Spawner;
use embassy_rp::gpio;
use embassy_rp::gpio::Input;
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi;
use embassy_rp::spi::Spi;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Delay;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    image::Image,
    mono_font::{ascii::*, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_text::{
    alignment::HorizontalAlignment,
    style::{HeightMode, TextBoxStyleBuilder},
    TextBox,
};
use gpio::{Level, Output, Pull};
use static_cell::StaticCell;
use uc8151::asynch::Uc8151;
use uc8151::LUT;
use uc8151::WIDTH;
use {defmt_rtt as _, panic_probe as _};

mod cyw43_driver;

type Spi0Bus = Mutex<NoopRawMutex, Spi<'static, SPI0, spi::Async>>;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let (_device, mut control) = setup_cyw43(
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

    let delay: Duration = Duration::from_secs(1);
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
    let spi_dev = SpiDevice::new(&spi_bus, cs);
    let mut display = Uc8151::new(spi_dev, dc, busy, reset, Delay);
    info!("Resetting display");
    display.reset().await;

    // Initialise display. Using the default LUT speed setting
    let test = display.setup(LUT::Internal).await;
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
    let bounds = Rectangle::new(Point::new(10, 10), Size::new(WIDTH - 157, 0));
    bounds
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(&mut display)
        .unwrap();

    // Create the text box and apply styling options.
    let text = "Hello BlueSky";
    let text_box = TextBox::with_textbox_style(text, bounds, character_style, textbox_style);

    // Draw the text box.
    text_box.draw(&mut display).unwrap();
    let _ = display.update().await;

    loop {
        Timer::after(delay).await;
    }
}
