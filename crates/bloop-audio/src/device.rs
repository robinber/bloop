//! Input device choice: the built-in microphone replaces a Bluetooth default input.
//!
//! When a Bluetooth headset connects, macOS makes it the default input. Its microphone goes through
//! the hands-free profile: a low sample rate, a switch of the headset audio at each capture start,
//! and no text from Voxtral for short dictations on the reference Mac. Thus the capture uses the
//! built-in microphone of the Mac when the default input is a Bluetooth device.

use coreaudio::audio_unit::Scope;
use coreaudio::audio_unit::macos_helpers::{
    get_audio_device_ids, get_audio_device_supports_scope, get_default_device_id, get_device_name,
    get_device_transport_type,
};

/// The transport of an audio device, from `kAudioDevicePropertyTransportType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    BuiltIn,
    Bluetooth,
    Other,
}

impl Transport {
    /// Maps the four-character code of Core Audio: `bltn`, `blue`, and `blea` are known.
    fn from_code(code: u32) -> Self {
        const BUILT_IN: u32 = u32::from_be_bytes(*b"bltn");
        const BLUETOOTH: u32 = u32::from_be_bytes(*b"blue");
        const BLUETOOTH_LOW_ENERGY: u32 = u32::from_be_bytes(*b"blea");
        match code {
            BUILT_IN => Self::BuiltIn,
            BLUETOOTH | BLUETOOTH_LOW_ENERGY => Self::Bluetooth,
            _ => Self::Other,
        }
    }
}

/// An input device of the system.
struct Input {
    transport: Transport,
    name: String,
}

/// Returns the name of the input device that replaces the default input, when one applies.
///
/// It returns the built-in microphone when the default input is a Bluetooth device.
/// Otherwise, and when a Core Audio read fails, it returns `None`: the capture uses the default input.
pub(crate) fn preferred_input() -> Option<String> {
    let default = get_default_device_id(true)?;
    let default = Transport::from_code(read(get_device_transport_type(default), "transport")?);
    let ids = read(get_audio_device_ids(), "devices")?;
    let name = choose(default, ids.into_iter().filter_map(input))?;
    tracing::info!(
        device = %name,
        "the default input is a Bluetooth device, the capture uses the built-in microphone"
    );
    Some(name)
}

/// The built-in microphone when `default` is a Bluetooth device, else nothing.
fn choose(default: Transport, inputs: impl IntoIterator<Item = Input>) -> Option<String> {
    if default != Transport::Bluetooth {
        return None;
    }
    inputs
        .into_iter()
        .find(|input| input.transport == Transport::BuiltIn)
        .map(|input| input.name)
}

/// Reads the transport and the name of a device that has input channels.
fn input(id: u32) -> Option<Input> {
    if !read(get_audio_device_supports_scope(id, Scope::Input), "streams")? {
        return None;
    }
    Some(Input {
        transport: Transport::from_code(read(get_device_transport_type(id), "transport")?),
        name: read(get_device_name(id), "name")?,
    })
}

/// Logs a failed Core Audio read of `property`. The capture then uses the default input.
fn read<T>(result: Result<T, coreaudio::Error>, property: &'static str) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(%error, property, "Core Audio did not answer, the capture uses the default input");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Input, Transport, choose};

    fn input(transport: Transport, name: &str) -> Input {
        Input {
            transport,
            name: name.to_owned(),
        }
    }

    #[test]
    fn the_transport_codes_follow_core_audio() {
        assert_eq!(Transport::from_code(0x626c_746e), Transport::BuiltIn);
        assert_eq!(Transport::from_code(0x626c_7565), Transport::Bluetooth);
        assert_eq!(Transport::from_code(0x626c_6561), Transport::Bluetooth);
        assert_eq!(Transport::from_code(0x7573_6220), Transport::Other);
    }

    #[test]
    fn the_built_in_microphone_replaces_a_bluetooth_default_input() {
        let inputs = [
            input(Transport::Bluetooth, "AirPods"),
            input(Transport::Other, "USB Microphone"),
            input(Transport::BuiltIn, "MacBook Pro Microphone"),
        ];

        assert_eq!(
            choose(Transport::Bluetooth, inputs).as_deref(),
            Some("MacBook Pro Microphone")
        );
    }

    #[test]
    fn a_bluetooth_default_input_stays_without_a_built_in_microphone() {
        let inputs = [
            input(Transport::Bluetooth, "AirPods"),
            input(Transport::Other, "USB Microphone"),
        ];

        assert_eq!(choose(Transport::Bluetooth, inputs), None);
    }

    #[test]
    fn another_default_input_stays() {
        let inputs = || {
            [
                input(Transport::BuiltIn, "MacBook Pro Microphone"),
                input(Transport::Bluetooth, "AirPods"),
            ]
        };

        assert_eq!(choose(Transport::BuiltIn, inputs()), None);
        assert_eq!(choose(Transport::Other, inputs()), None);
    }
}
