use core::{fmt, hint::black_box};

use fugit::HertzU32 as Hertz;
use sdio_host::{
    common_cmd::{self, ResponseLen},
    sd::{CardCapacity, CardStatus, CurrentState, SDStatus, CIC, CID, CSD, OCR, RCA, SCR, SD},
    sd_cmd, Cmd,
};

use crate::{
    gpio::{self, Alternate, PushPull},
    pac::{sdmmc1, SDMMC1},
    rcc::{Clocks, Enable, Reset, APB2},
};

pub trait PinClk {}
pub trait PinCmd {}
pub trait PinD0 {}
pub trait PinD1 {}
pub trait PinD2 {}
pub trait PinD3 {}
pub trait PinD4 {}
pub trait PinD5 {}
pub trait PinD6 {}
pub trait PinD7 {}

pub trait Pins {
    const BUS_WIDTH: BusWidth;
}

impl<CLK, CMD, D0> Pins for (CLK, CMD, D0)
where
    CLK: PinClk,
    CMD: PinCmd,
    D0: PinD0,
{
    const BUS_WIDTH: BusWidth = BusWidth::One;
}

impl<CLK, CMD, D0, D1, D2, D3> Pins for (CLK, CMD, D0, D1, D2, D3)
where
    CLK: PinClk,
    CMD: PinCmd,
    D0: PinD0,
    D1: PinD1,
    D2: PinD2,
    D3: PinD3,
{
    const BUS_WIDTH: BusWidth = BusWidth::Four;
}

impl<CLK, CMD, D0, D1, D2, D3, D4, D5, D6, D7> Pins for (CLK, CMD, D0, D1, D2, D3, D4, D5, D6, D7)
where
    CLK: PinClk,
    CMD: PinCmd,
    D0: PinD0,
    D1: PinD1,
    D2: PinD2,
    D3: PinD3,
    D4: PinD4,
    D5: PinD5,
    D6: PinD6,
    D7: PinD7,
{
    const BUS_WIDTH: BusWidth = BusWidth::Eight;
}

macro_rules! pins {
    ($(CLK: [$($CLK:ty),*] CMD: [$($CMD:ty),*] D0: [$($D0:ty),*] D1: [$($D1:ty),*] D2: [$($D2:ty),*] D3: [$($D3:ty),*] D4: [$($D4:ty),*] D5: [$($D5:ty),*] D6: [$($D6:ty),*] D7: [$($D7:ty),*])+) => {
        $(
            $(
                impl PinClk for $CLK {}
            )*
            $(
                impl PinCmd for $CMD {}
            )*
            $(
                impl PinD0 for $D0 {}
            )*
            $(
                impl PinD1 for $D1 {}
            )*
            $(
                impl PinD2 for $D2 {}
            )*
            $(
                impl PinD3 for $D3 {}
            )*
            $(
                impl PinD4 for $D4 {}
            )*
            $(
                impl PinD5 for $D5 {}
            )*
            $(
                impl PinD6 for $D6 {}
            )*
            $(
                impl PinD7 for $D7 {}
            )*
        )+
    }
}

#[cfg(any(feature = "stm32l496",))]
pins! {
  CLK: [gpio::PC12<Alternate<PushPull, 12>>]
  CMD: [gpio::PD2<Alternate<PushPull, 12>>]
  D0: [gpio::PC8<Alternate<PushPull, 12>>]
  D1: [gpio::PC9<Alternate<PushPull, 12>>]
  D2: [gpio::PC10<Alternate<PushPull, 12>>]
  D3: [gpio::PC11<Alternate<PushPull, 12>>]
  D4: [gpio::PB8<Alternate<PushPull, 12>>]
  D5: [gpio::PB9<Alternate<PushPull, 12>>]
  D6: [gpio::PC6<Alternate<PushPull, 12>>]
  D7: [gpio::PC7<Alternate<PushPull, 12>>]
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
pub enum BusWidth {
    One = 0,
    Four = 1,
    Eight = 2,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
pub enum ClockFreq {
    Freq24MHz = 0,
    Freq16MHz = 1,
    Freq12MHz = 2,
    Freq8MHz = 8,
    Freq4MHz = 10,
    Freq1MHz = 46,
    Freq400KHz = 118,
}

#[derive(Debug)]
pub enum Error {
    NoCard,
    SoftwareTimeout,
    Crc,
    DataCrcFail,
    RxOverFlow,
    Timeout,
    TxUnderErr,
    RespCmdMismatch,
}

macro_rules! try_datapath {
    ($sta:ident) => {
        if $sta.rxoverr().bit() {
            return Err(Error::RxOverFlow);
        }

        if $sta.dcrcfail().bit() {
            return Err(Error::DataCrcFail);
        }

        if $sta.dtimeout().bit() {
            return Err(Error::Timeout);
        }
    };
}

fn clear_all_interrupts(icr: &sdmmc1::ICR) {
    icr.modify(|_, w| {
        w.ccrcfailc()
            .set_bit()
            .ctimeoutc()
            .set_bit()
            .ceataendc()
            .set_bit()
            .cmdrendc()
            .set_bit()
            .cmdsentc()
            .set_bit()
            .dataendc()
            .set_bit()
            .dbckendc()
            .set_bit()
            .dcrcfailc()
            .set_bit()
            .dtimeoutc()
            .set_bit()
            .sdioitc()
            .set_bit()
            .stbiterrc()
            .set_bit()
            .rxoverrc()
            .set_bit()
            .txunderrc()
            .set_bit()
    });
}

#[derive(Default)]
pub struct SdCard {
    capacity: CardCapacity,
    ocr: OCR<SD>,
    cid: CID<SD>,
    rca: RCA<SD>,
    csd: CSD<SD>,
    scr: SCR,
}

/// TODO:  Remove when https://github.com/jkristell/sdio-host/issues/11 is fixed.
impl fmt::Debug for SdCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SdCard")
            .field("capacity", &self.capacity)
            .field("cid", &self.cid)
            .field("csd", &self.csd)
            .field("ocr", &self.ocr)
            .field("rca", &self.rca.address())
            .field("scr", &self.scr)
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
/// Indicates transfer direction
enum Dir {
    HostToCard = 0,
    CardToHost = 1,
}

impl SdCard {
    pub fn get_address(&self) -> u16 {
        self.rca.address()
    }

    pub fn supports_widebus(&self) -> bool {
        self.scr.bus_width_four()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SdCardType {
    Sdsc,
    SdhcSdxc,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SdCardVersion {
    V1,
    V2,
}

#[derive(Debug)]
pub struct SdMmc {
    sdmmc: SDMMC1,
    clock: Hertz,
    bus_width: BusWidth,
    card: Option<SdCard>,
}

impl SdMmc {
    pub fn new<PINS: Pins>(
        mut sdmmc: SDMMC1,
        _pins: PINS,
        apb2: &mut APB2,
        clocks: &Clocks,
    ) -> Self {
        SDMMC1::enable(apb2);
        SDMMC1::reset(apb2);

        let clock = clocks.sysclk();

        sdmmc.clkcr.modify(|_, w| unsafe {
            w.negedge()
                .clear_bit() // Rising Edge
                .bypass()
                .clear_bit() // Disable bypass.
                .pwrsav()
                .clear_bit() // Disable power-save.
                .widbus()
                .bits(0) // Bus Width 1
                .hwfc_en()
                .clear_bit() // Disable hardware flow-control.
                .clkdiv()
                .bits(ClockFreq::Freq400KHz as u8) // Clock must be <= 400 kHz while in identification mode.
                .clken()
                .clear_bit() // Disable clock.
        });

        let mut host = Self {
            sdmmc,
            clock,
            bus_width: PINS::BUS_WIDTH,
            card: None,
        };

        host.power_card(false);

        host
    }

    pub fn init(&mut self, freq: ClockFreq) -> Result<(), Error> {
        self.power_card(true);

        // Enable clock.
        self.sdmmc.clkcr.modify(|_, w| w.clken().set_bit());

        self.cmd(common_cmd::idle())?;

        let check_pattern = 0xaa;
        self.cmd(sd_cmd::send_if_cond(1, check_pattern))?;
        let cic = CIC::from(self.sdmmc.resp1.read().bits());

        let card_version = if cic.pattern() == 0xAA {
            SdCardVersion::V2
        } else {
            SdCardVersion::V1
        };

        let mut card = SdCard::default();

        let high_capacity = card_version == SdCardVersion::V2;
        let voltage_window = 1 << 5;

        let mut timeout = 0xffff;
        let ocr: OCR<SD> = loop {
            if timeout == 0 {
                return Err(Error::SoftwareTimeout);
            }

            timeout -= 1;

            match self.app_cmd(sd_cmd::sd_send_op_cond(
                high_capacity,
                false,
                false,
                voltage_window,
            )) {
                Ok(()) => (),
                Err(Error::Crc) => (),
                Err(err) => return Err(err),
            }

            let ocr = OCR::from(self.sdmmc.resp1.read().bits());
            if !ocr.is_busy() {
                // Power up done
                break ocr;
            }
        };

        card.capacity = if ocr.high_capacity() {
            CardCapacity::HighCapacity
        } else {
            CardCapacity::StandardCapacity
        };
        card.ocr = ocr;

        self.cmd(common_cmd::all_send_cid())?;
        card.cid = CID::from([
            self.sdmmc.resp1.read().bits(),
            self.sdmmc.resp2.read().bits(),
            self.sdmmc.resp3.read().bits(),
            self.sdmmc.resp4.read().bits(),
        ]);

        self.cmd(sd_cmd::send_relative_address())?;
        card.rca = RCA::from(self.sdmmc.resp1.read().bits());

        self.cmd(common_cmd::send_csd(card.rca.address()))?;
        card.csd = CSD::from([
            self.sdmmc.resp1.read().bits(),
            self.sdmmc.resp2.read().bits(),
            self.sdmmc.resp3.read().bits(),
            self.sdmmc.resp4.read().bits(),
        ]);

        self.select_card(card.rca.address())?;
        card.scr = self.get_scr(card.rca.address())?;

        //  `app_cmd` will select this card from now on.
        self.card = Some(card);

        // Wait before setting the bus witdth and frequency to avoid timeouts on SDSC cards
        while !self.card_ready()? {}

        self.set_bus(self.bus_width, freq)?;

        Ok(())
    }

    fn set_bus(&self, bus_width: BusWidth, freq: ClockFreq) -> Result<(), Error> {
        let card_widebus = self.card()?.supports_widebus();

        let bus_width = match bus_width {
            BusWidth::Eight | BusWidth::Four if card_widebus => BusWidth::Four,
            _ => BusWidth::One,
        };

        self.app_cmd(sd_cmd::set_bus_width(bus_width == BusWidth::Four))?;

        self.sdmmc.clkcr.modify(|_, w| unsafe {
            w.clkdiv()
                .bits(freq as u8)
                .widbus()
                .bits(bus_width as u8)
                .clken()
                .set_bit()
        });
        Ok(())
    }

    fn start_datapath_transfer(&self, length_bytes: u32, block_size: u8, direction: Dir) {
        // Block size up to 2^14 bytes
        assert!(block_size <= 14);

        loop {
            let sta = self.sdmmc.sta.read();

            if sta.cmdact().bit_is_clear()
                && sta.rxact().bit_is_clear()
                && sta.txact().bit_is_clear()
            {
                break;
            }
        }

        self.sdmmc
            .dtimer
            .modify(|_, w| unsafe { w.datatime().bits(0xFFFFFFFF) });

        self.sdmmc
            .dlen
            .modify(|_, w| unsafe { w.datalength().bits(length_bytes) });

        self.sdmmc.dctrl.modify(|_, w| unsafe {
            w.dblocksize()
                .bits(block_size)
                .dtdir()
                .bit(match direction {
                    Dir::HostToCard => false,
                    Dir::CardToHost => true,
                })
                .dtmode()
                .clear_bit()
                .dten()
                .set_bit()
        });
    }

    fn get_scr(&self, rca: u16) -> Result<SCR, Error> {
        self.cmd(common_cmd::set_block_length(8))?;
        self.cmd(common_cmd::app_cmd(rca))?;
        self.start_datapath_transfer(8, 3, Dir::CardToHost);
        self.cmd(sd_cmd::send_scr())?;

        let mut scr = [0x00; 2];

        'outer: for n in scr.iter_mut().rev() {
            loop {
                let sta = self.sdmmc.sta.read();

                try_datapath!(sta);

                if sta.dbckend().bit() {
                    break 'outer;
                }

                if sta.rxdavl().bit_is_set() {
                    *n = self.sdmmc.fifo.read().bits().swap_bytes();
                    continue 'outer;
                }
            }
        }

        return Ok(SCR::from(scr));
    }

    pub fn power_card(&mut self, on: bool) {
        self.sdmmc
            .power
            .modify(|_, w| unsafe { w.pwrctrl().bits(if on { 0b11 } else { 0b00 }) });

        // Wait for 2 ms after power mode change.
        cortex_m::asm::delay(2 * (self.clock.raw() / 1000));
    }

    fn read_status(&mut self) -> Result<CardStatus<SdCard>, Error> {
        let card = self.card()?;

        self.cmd(common_cmd::card_status(card.get_address(), false))?;

        let r1 = self.sdmmc.resp1.read().bits();
        Ok(CardStatus::from(r1))
    }

    /// Check if card is done writing/reading and back in transfer state
    fn card_ready(&mut self) -> Result<bool, Error> {
        Ok(self.read_status()?.state() == CurrentState::Transfer)
    }

    /// Read the SDStatus struct
    pub fn read_sd_status(&mut self) -> Result<SDStatus, Error> {
        self.card()?;
        self.cmd(common_cmd::set_block_length(64))?;
        self.start_datapath_transfer(64, 6, Dir::CardToHost);
        self.app_cmd(sd_cmd::sd_status())?;

        let mut sd_status = [0u32; 16];

        'outer: for i in 0..2 {
            loop {
                let sta = self.sdmmc.sta.read();

                try_datapath!(sta);

                if sta.dbckend().bit() {
                    break 'outer;
                }

                if sta.rxfifohf().bit() {
                    for j in 0..8 {
                        sd_status[15 - i * 8 - j] = self.sdmmc.fifo.read().bits().swap_bytes();
                    }

                    continue 'outer;
                }
            }
        }

        Ok(SDStatus::from(sd_status))
    }

    pub fn card(&self) -> Result<&SdCard, Error> {
        self.card.as_ref().ok_or(Error::NoCard)
    }

    fn select_card(&self, rca: u16) -> Result<(), Error> {
        let r = self.cmd(common_cmd::select_card(rca));
        match (r, rca) {
            (Err(Error::Timeout), 0) => Ok(()),
            (r, _) => r,
        }
    }

    pub fn app_cmd<R: common_cmd::Resp>(&self, cmd: Cmd<R>) -> Result<(), Error> {
        let rca = self.card().map(|card| card.get_address()).unwrap_or(0);
        self.cmd(common_cmd::app_cmd(rca))?;
        self.cmd(cmd)
    }

    pub fn cmd<R: common_cmd::Resp>(&self, cmd: Cmd<R>) -> Result<(), Error> {
        while self.sdmmc.sta.read().cmdact().bit_is_set() {}

        // Clear the interrupts before we start
        clear_all_interrupts(&self.sdmmc.icr);

        self.sdmmc
            .arg
            .write(|w| unsafe { w.cmdarg().bits(cmd.arg) });

        let waitresp = match cmd.response_len() {
            ResponseLen::Zero => 0b00,
            ResponseLen::R48 => 0b01,
            ResponseLen::R136 => 0b11,
        };

        self.sdmmc.cmd.write(|w| unsafe {
            w.cmdindex()
                .bits(cmd.cmd)
                .waitresp()
                .bits(waitresp)
                .waitint()
                .clear_bit()
                .cpsmen()
                .set_bit()
        });

        let timeout = 5000 * (self.clock.raw() / 8 / 1000);
        for _ in 0..timeout {
            let sta = self.sdmmc.sta.read();

            if sta.cmdact().bit_is_set() {
                // Command transfer still in progress.
                continue;
            }

            if cmd.response_len() == ResponseLen::Zero {
                if sta.ctimeout().bit() {
                    return Err(Error::Timeout);
                }

                if sta.cmdsent().bit() {
                    return Ok(());
                }
            } else {
                if sta.ctimeout().bit() {
                    return Err(Error::Timeout);
                }

                if sta.ccrcfail().bit() {
                    return Err(Error::Crc);
                }

                if sta.cmdrend().bit() {
                    return Ok(());
                }
            }
        }

        Err(Error::SoftwareTimeout)
    }
}
