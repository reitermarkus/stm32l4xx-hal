//! Power management

use crate::rcc::{Clocks, Enable, APB1R1};
use crate::stm32::{pwr, PWR};
use bitfield::{bitfield, BitRange};
use cortex_m::peripheral::SCB;
use fugit::RateExtU32;

/// PWR error
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// Power regulator con not be switched to the low-power voltage due to the system clock frequency being higher than 26MHz
    SysClkTooHighVos,
    /// System can not be switched to the low-power run mode due to the system clock frequency being higher than 2MHz
    SysClkTooHighLpr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VosRange {
    #[doc = "High-Performance range, 1.2V, up to 80 MHz"]
    HighPerformance = 0b01,
    #[doc = "Low-power range, 1.0V, up to 26MHz"]
    LowPower = 0b10,
}

bitfield! {
  pub struct WakeUpSource(u16);
  impl Debug;
  // The fields default to u16
  pub wkup1, set_wkup1: 0;
  pub wkup2, set_wkup2: 1;
  pub wkup3, set_wkup3: 2;
  pub wkup4, set_wkup4: 3;
  pub wkup5, set_wkup5: 4;
  pub internal_wkup, set_internal_wkup: 15;
}

enum LowPowerMode {
  Stop0    = 0b000,
  Stop1    = 0b001,
  Stop2    = 0b010,
  Standby  = 0b011,
  Shutdown = 0b100, // 0b1xx
}

#[must_use = "`wait_for_interrupt` or `wait_for_event` must be called to enter low-power mode."]
pub struct LowPowerModeGuard {
  _0: ()
}

impl LowPowerModeGuard {
  /// Wait for an interrupt. Must not be called from within a critical section.
  #[inline]
  pub fn wait_for_interrupt(self) {
    cortex_m::asm::dsb();
    cortex_m::asm::wfi();
  }

  /// Wait for an event. Must not be called from within a critical section.
  #[inline]
  pub fn wait_for_event(self) {
    cortex_m::asm::dsb();
    cortex_m::asm::sev();
    cortex_m::asm::wfe();
    cortex_m::asm::wfe();
  }
}

pub struct Pwr {
    pub cr1: CR1,
    pub cr2: CR2,
    pub cr3: CR3,
    pub cr4: CR4,
    pub scr: SCR,
    pub sr1: SR1,
}

impl Pwr {
    /// Configures dynamic voltage regulator range
    ///
    /// Will panic if low-power range is selected for higher system clock
    pub fn set_power_range(&mut self, range: VosRange, clocks: &Clocks) -> Result<(), Error> {
        match range {
            VosRange::HighPerformance => unsafe {
                {
                    self.cr1
                        .reg()
                        .modify(|_, w| w.vos().bits(VosRange::HighPerformance as u8))
                }
                Ok(())
            },
            VosRange::LowPower => {
                if clocks.sysclk() > 26.MHz::<1, 1>() {
                    Err(Error::SysClkTooHighVos)
                } else {
                    unsafe {
                        self.cr1
                            .reg()
                            .modify(|_, w| w.vos().bits(VosRange::LowPower as u8))
                    }
                    Ok(())
                }
            }
        }
    }

    /// Switches the system into low power run mode
    pub fn low_power_run(&mut self, clocks: &Clocks) -> Result<(), Error> {
        if clocks.sysclk() > 2.MHz::<1, 1>() {
            Err(Error::SysClkTooHighLpr)
        } else {
            self.cr1.reg().modify(|_, w| w.lpr().set_bit());
            Ok(())
        }
    }

    #[inline]
    fn enter_low_power_mode(&mut self, mode: LowPowerMode, scb: &mut SCB) -> LowPowerModeGuard {
        unsafe { self.cr1.reg().modify(|_, w| w.lpms().bits(mode as u8)) };
        scb.set_sleepdeep();

        LowPowerModeGuard { _0: () }
    }

    /// Enter “Stop 0” low power mode.
    pub fn stop0(&mut self, scb: &mut SCB) -> LowPowerModeGuard {
      self.enter_low_power_mode(LowPowerMode::Stop0, scb)
    }

    /// Enter “Stop 1” low power mode.
    pub fn stop1(&mut self, scb: &mut SCB) -> LowPowerModeGuard {
      self.enter_low_power_mode(LowPowerMode::Stop1, scb)
    }

    /// Enter “Stop 2” low power mode.
    pub fn stop2(&mut self, scb: &mut SCB) -> LowPowerModeGuard {
      self.enter_low_power_mode(LowPowerMode::Stop2, scb)
    }

    fn enter_shutdown_or_standby(&mut self, mode: LowPowerMode, wkup: &WakeUpSource, scb: &mut SCB) -> LowPowerModeGuard {
      unsafe {
        self.cr3.reg().modify(|_, w| w.bits(wkup.bit_range(0, 7)));
      }

      if wkup.internal_wkup() {
          // Can't apply directly due to the APC and RPS bits
          self.cr3.reg().modify(|_, w| w.ewf().set_bit())
      }
      self.scr.reg().write(|w| {
          w.wuf1()
              .set_bit()
              .wuf2()
              .set_bit()
              .wuf3()
              .set_bit()
              .wuf4()
              .set_bit()
              .wuf5()
              .set_bit()
              .sbf()
              .set_bit()
      });

      self.enter_low_power_mode(mode, scb)
    }

    /// Enters “Standby” low power mode.
    pub fn standby(&mut self, wkup: &WakeUpSource, scb: &mut SCB) -> LowPowerModeGuard {
      self.enter_shutdown_or_standby(LowPowerMode::Standby, wkup, scb)
    }

    /// Enters “Shutdown” low power mode.
    pub fn shutdown(&mut self, wkup: &WakeUpSource, scb: &mut SCB) -> LowPowerModeGuard {
      self.enter_shutdown_or_standby(LowPowerMode::Shutdown, wkup, scb)
    }

    /// Returns the reason, why wakeup from shutdown happened. In case there is more than one,
    /// a single random reason will be returned.
    pub fn read_wakeup_reason(&mut self) -> WakeUpSource {
        WakeUpSource(self.sr1.reg().read().bits() as u16)
    }
}

/// Extension trait that constrains the `PWR` peripheral
pub trait PwrExt {
    /// Constrains the `PWR` peripheral so it plays nicely with the other abstractions
    fn constrain(self, _: &mut APB1R1) -> Pwr;
}

impl PwrExt for PWR {
    fn constrain(self, apb1r1: &mut APB1R1) -> Pwr {
        // Enable the peripheral clock
        PWR::enable(apb1r1);
        Pwr {
            cr1: CR1 { _0: () },
            cr2: CR2 { _0: () },
            cr3: CR3 { _0: () },
            cr4: CR4 { _0: () },
            scr: SCR { _0: () },
            sr1: SR1 { _0: () },
        }
    }
}

/// CR1
pub struct CR1 {
    _0: (),
}

impl CR1 {
    pub(crate) fn reg(&mut self) -> &pwr::CR1 {
        // NOTE(unsafe) this proxy grants exclusive access to this register
        unsafe { &(*PWR::ptr()).cr1 }
    }
}
/// CR2
pub struct CR2 {
    _0: (),
}

impl CR2 {
    // TODO remove `allow`
    #[allow(dead_code)]
    pub(crate) fn reg(&mut self) -> &pwr::CR2 {
        // NOTE(unsafe) this proxy grants exclusive access to this register
        unsafe { &(*PWR::ptr()).cr2 }
    }
}
/// CR3
pub struct CR3 {
    _0: (),
}

impl CR3 {
    pub(crate) fn reg(&mut self) -> &pwr::CR3 {
        // NOTE(unsafe) this proxy grants exclusive access to this register
        unsafe { &(*PWR::ptr()).cr3 }
    }
}
/// CR4
pub struct CR4 {
    _0: (),
}

impl CR4 {
    // TODO remove `allow`
    #[allow(dead_code)]
    pub(crate) fn reg(&mut self) -> &pwr::CR4 {
        // NOTE(unsafe) this proxy grants exclusive access to this register
        unsafe { &(*PWR::ptr()).cr4 }
    }
}

/// SCR
pub struct SCR {
    _0: (),
}

impl SCR {
    pub(crate) fn reg(&mut self) -> &pwr::SCR {
        // NOTE(unsafe) this proxy grants exclusive access to this register
        unsafe { &(*PWR::ptr()).scr }
    }
}

/// SCR
pub struct SR1 {
    _0: (),
}

impl SR1 {
    pub(crate) fn reg(&mut self) -> &pwr::SR1 {
        // NOTE(unsafe) this proxy grants exclusive access to this register
        unsafe { &(*PWR::ptr()).sr1 }
    }
}
