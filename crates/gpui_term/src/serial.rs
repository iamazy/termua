use anyhow::Context as _;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SerialOptions {
    pub port: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: SerialParity,
    pub stop_bits: SerialStopBits,
    pub flow_control: SerialFlowControl,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SerialParity {
    None,
    Even,
    Odd,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SerialStopBits {
    One,
    Two,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SerialFlowControl {
    None,
    Software,
    Hardware,
}

pub(crate) fn serial2_char_size(data_bits: u8) -> anyhow::Result<serial2::CharSize> {
    Ok(match data_bits {
        5 => serial2::CharSize::Bits5,
        6 => serial2::CharSize::Bits6,
        7 => serial2::CharSize::Bits7,
        8 => serial2::CharSize::Bits8,
        other => anyhow::bail!("unsupported serial data_bits={other}"),
    })
}

pub(crate) fn serial2_parity(parity: SerialParity) -> serial2::Parity {
    match parity {
        SerialParity::None => serial2::Parity::None,
        SerialParity::Even => serial2::Parity::Even,
        SerialParity::Odd => serial2::Parity::Odd,
    }
}

pub(crate) fn serial2_stop_bits(bits: SerialStopBits) -> serial2::StopBits {
    match bits {
        SerialStopBits::One => serial2::StopBits::One,
        SerialStopBits::Two => serial2::StopBits::Two,
    }
}

pub(crate) fn serial2_flow_control(flow: SerialFlowControl) -> serial2::FlowControl {
    match flow {
        SerialFlowControl::None => serial2::FlowControl::None,
        SerialFlowControl::Software => serial2::FlowControl::XonXoff,
        SerialFlowControl::Hardware => serial2::FlowControl::RtsCts,
    }
}

pub(crate) fn apply_serial_options_to_serial2_settings(
    settings: &mut serial2::Settings,
    opts: &SerialOptions,
) -> anyhow::Result<()> {
    settings
        .set_baud_rate(opts.baud)
        .with_context(|| format!("set baud rate {}", opts.baud))?;
    settings.set_raw();
    settings.set_char_size(serial2_char_size(opts.data_bits)?);
    settings.set_parity(serial2_parity(opts.parity));
    settings.set_stop_bits(serial2_stop_bits(opts.stop_bits));
    settings.set_flow_control(serial2_flow_control(opts.flow_control));
    Ok(())
}
