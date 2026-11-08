//! Input device choice: the built-in microphone replaces a Bluetooth default input.
//!
//! When a Bluetooth headset connects, macOS makes it the default input. Its microphone goes through
//! the hands-free profile: a low sample rate, a switch of the headset audio at each capture start,
//! and short dictations that can give no text from the speech provider. Thus the capture uses the
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

/// The input device of a capture.
#[derive(Debug, PartialEq, Eq)]
enum Choice {
    /// The default input.
    Default,
    /// The built-in microphone with this name, in place of a Bluetooth default input.
    BuiltIn(String),
    /// The default input is a Bluetooth device, and no input device read as built-in.
    NoBuiltIn,
}

/// Returns the name of the input device that replaces the default input, when one applies.
///
/// It returns the built-in microphone when the default input is a Bluetooth device.
/// Otherwise, and when the default input or the device list cannot be read, it returns `None`: the capture uses
/// the default input. A device that cannot be read is left out of the choice.
pub(crate) fn preferred_input() -> Option<String> {
    let Some(default) = get_default_device_id(true) else {
        tracing::warn!(
            property = "default input",
            "Core Audio did not answer, the capture uses the default input"
        );
        return None;
    };
    let Some(transport) = read(get_device_transport_type(default), "transport") else {
        tracing::warn!(
            "without the transport of the default input, the capture uses the default input"
        );
        return None;
    };
    let default = Transport::from_code(transport);
    let inputs =
        || read(get_audio_device_ids(), "devices").map(|ids| ids.into_iter().filter_map(input));
    match choose(default, inputs) {
        Choice::Default => None,
        Choice::BuiltIn(name) => {
            tracing::info!(
                device = %name,
                "the default input is a Bluetooth device, the capture uses the built-in microphone"
            );
            Some(name)
        }
        Choice::NoBuiltIn => {
            tracing::warn!(
                "the default input is a Bluetooth device and no built-in microphone was found, \
                 the capture uses the default input"
            );
            None
        }
    }
}

/// Chooses the input device for a default input with the `default` transport.
///
/// It calls `inputs` only when `default` is a Bluetooth device. Without the list, the capture
/// uses the default input.
fn choose<I>(default: Transport, inputs: impl FnOnce() -> Option<I>) -> Choice
where
    I: IntoIterator<Item = Input>,
{
    if default != Transport::Bluetooth {
        return Choice::Default;
    }
    let Some(inputs) = inputs() else {
        tracing::warn!("without the device list, the capture uses the default input");
        return Choice::Default;
    };
    inputs
        .into_iter()
        .find(|input| input.transport == Transport::BuiltIn)
        .map_or(Choice::NoBuiltIn, |input| Choice::BuiltIn(input.name))
}

/// Reads the transport and the name of a device that has input channels. A failed read skips the device.
fn input(id: u32) -> Option<Input> {
    let Some(streams) = read(get_audio_device_supports_scope(id, Scope::Input), "streams") else {
        return skip(id);
    };
    if !streams {
        return None;
    }
    let Some(transport) = read(get_device_transport_type(id), "transport") else {
        return skip(id);
    };
    let Some(name) = read(get_device_name(id), "name") else {
        return skip(id);
    };
    Some(Input {
        transport: Transport::from_code(transport),
        name,
    })
}

/// Logs that the device choice leaves out the device `id`, after a failed read.
fn skip(id: u32) -> Option<Input> {
    tracing::warn!(
        device = id,
        "the device choice skips a device that it cannot read"
    );
    None
}

/// Logs a failed Core Audio read of `property`. The caller decides what happens without the value.
fn read<T>(result: Result<T, coreaudio::Error>, property: &'static str) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(%error, property, "a Core Audio read failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Choice, Input, Transport, choose};

    fn input(transport: Transport, name: &str) -> Input {
        Input {
            transport,
            name: name.to_owned(),
        }
    }

    fn unread() -> Option<Vec<Input>> {
        panic!("the device list is read only for a Bluetooth default input")
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
            choose(Transport::Bluetooth, || Some(inputs)),
            Choice::BuiltIn("MacBook Pro Microphone".to_owned())
        );
    }

    #[test]
    fn a_bluetooth_default_input_stays_without_a_built_in_microphone() {
        let inputs = [
            input(Transport::Bluetooth, "AirPods"),
            input(Transport::Other, "USB Microphone"),
        ];

        assert_eq!(
            choose(Transport::Bluetooth, || Some(inputs)),
            Choice::NoBuiltIn
        );
    }

    #[test]
    fn a_bluetooth_default_input_stays_without_the_device_list() {
        assert_eq!(
            choose(Transport::Bluetooth, || None::<Vec<Input>>),
            Choice::Default
        );
    }

    #[test]
    fn another_default_input_stays() {
        assert_eq!(choose(Transport::BuiltIn, unread), Choice::Default);
        assert_eq!(choose(Transport::Other, unread), Choice::Default);
    }
}
