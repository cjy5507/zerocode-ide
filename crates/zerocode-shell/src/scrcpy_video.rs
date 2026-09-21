//! scrcpy's video stream, as bytes on a wire.
//!
//! Framing only: no sockets, no processes, no adb. The socket reader hands
//! chunks in and gets whole packets out, which is what makes the one part of
//! this protocol that can be wrong in silence — where a packet ends — testable
//! without a phone.
//!
//! The layout is the original's, and it is pinned to the server version the
//! original pinned (`SCRCPY_SERVER_VERSION = '2.4'`; this window now pins 3.3.4, same framing, with `send_codec_meta` and
//! `send_frame_meta` left on):
//!
//! - a one-byte readiness marker, then a 64-byte device name, once
//!   (`DEVICE_NAME_BYTES = 64`, `DUMMY_BYTE = 1`, scrcpy-stream-session.ts)
//! - twelve bytes of codec metadata, once: a four-character codec id padded
//!   with NULs, then the initial width and height as big-endian u32
//! - then packets forever: eight big-endian bytes whose top two bits are
//!   flags and whose remaining sixty-two are a presentation timestamp, four
//!   big-endian bytes of length, and that many bytes of H.264
//!
//! The payload bytes are Annex-B NAL units — the same shape `screenrecord`
//! writes — which is why the window's decoder needs to learn nothing about
//! any of this.

/// The one-byte readiness marker and the fixed-width device name that open the
/// video socket.
const HANDSHAKE_BYTES: usize = 1 + 64;

/// The codec header: four characters of codec id, then width and height.
const CODEC_META_BYTES: usize = 12;

/// Per packet: eight bytes of flags-and-timestamp, four of length.
const PACKET_HEADER_BYTES: usize = 12;

/// A packet larger than this is not a large packet, it is a stream that has
/// lost its place — every frame at the size this window asks for is orders
/// smaller. The original refuses at the same number rather than buffering
/// towards an out-of-memory kill.
const MAX_PACKET_BYTES: u32 = 16 * 1024 * 1024;

/// The top bit of the timestamp word: this packet is the codec configuration
/// (SPS and PPS), not a picture.
const CONFIG_FLAG: u64 = 1 << 63;

/// The bit below it: this picture can be decoded without any before it.
const KEY_FRAME_FLAG: u64 = 1 << 62;

/// What is left of the word once the two flags are taken off it.
const PTS_MASK: u64 = (1 << 62) - 1;

/// What the server said it was about to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecMeta {
    /// `"h264"` for the codec this window asks for. Kept as text rather than
    /// matched here, because a reader that only says "not what I expected" is
    /// worth more than one that says "no".
    pub codec: String,
    pub width: u32,
    pub height: u32,
}

/// One packet of the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// The parameter sets, which arrive before any picture and again after a
    /// rotation.
    pub config: bool,
    pub key_frame: bool,
    pub pts: u64,
    pub data: Vec<u8>,
}

/// The stream stopped making sense, and the only honest thing left is to close
/// it: a length field that cannot be true means the reader has lost the frame
/// boundary, and every byte after it would be garbage handed to a decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desynced {
    pub said: u32,
}

impl std::fmt::Display for Desynced {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "scrcpy frame of {} bytes is past {MAX_PACKET_BYTES}; the stream lost its place",
            self.said
        )
    }
}

/// Read the opening handshake out of `held`, leaving whatever came after it.
///
/// Answers `None` while the handshake is still arriving — a socket hands over
/// whatever has landed, and 65 bytes is not promised in one read.
pub fn take_handshake(held: &mut Vec<u8>) -> Option<String> {
    if held.len() < HANDSHAKE_BYTES {
        return None;
    }
    let rest = held.split_off(HANDSHAKE_BYTES);
    let name = held[1..HANDSHAKE_BYTES]
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| char::from(*byte))
        .collect();
    *held = rest;
    Some(name)
}

/// Read the codec header out of `held`, leaving the packet stream behind it.
pub fn take_codec_meta(held: &mut Vec<u8>) -> Option<CodecMeta> {
    if held.len() < CODEC_META_BYTES {
        return None;
    }
    let rest = held.split_off(CODEC_META_BYTES);
    let codec = held[..4]
        .iter()
        .filter(|byte| **byte != 0)
        .map(|byte| char::from(*byte))
        .collect();
    let width = u32::from_be_bytes([held[4], held[5], held[6], held[7]]);
    let height = u32::from_be_bytes([held[8], held[9], held[10], held[11]]);
    *held = rest;
    Some(CodecMeta {
        codec,
        width,
        height,
    })
}

/// Take every whole packet out of `held`, leaving the tail of a partial one.
///
/// The tail is the whole point: a socket read ends wherever the network felt
/// like ending it, and a parser that assumed otherwise would drop a frame
/// every time one straddled two reads.
///
/// # Errors
///
/// [`Desynced`] when a length field is past [`MAX_PACKET_BYTES`]. Nothing is
/// consumed in that case: the caller's next move is to close the stream, not
/// to carry on from a place it can no longer trust.
pub fn take_packets(held: &mut Vec<u8>) -> Result<Vec<Packet>, Desynced> {
    let mut packets = Vec::new();
    let mut at = 0usize;
    while held.len() - at >= PACKET_HEADER_BYTES {
        let word = u64::from_be_bytes([
            held[at],
            held[at + 1],
            held[at + 2],
            held[at + 3],
            held[at + 4],
            held[at + 5],
            held[at + 6],
            held[at + 7],
        ]);
        let size = u32::from_be_bytes([held[at + 8], held[at + 9], held[at + 10], held[at + 11]]);
        if size > MAX_PACKET_BYTES {
            return Err(Desynced { said: size });
        }
        let body = at + PACKET_HEADER_BYTES;
        let size = size as usize;
        if held.len() - body < size {
            break;
        }
        packets.push(Packet {
            config: word & CONFIG_FLAG != 0,
            key_frame: word & KEY_FRAME_FLAG != 0,
            pts: word & PTS_MASK,
            data: held[body..body + size].to_vec(),
        });
        at = body + size;
    }
    held.drain(..at);
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::{CodecMeta, Packet, take_codec_meta, take_handshake, take_packets};

    fn packet(word: u64, body: &[u8]) -> Vec<u8> {
        let mut bytes = word.to_be_bytes().to_vec();
        #[allow(clippy::cast_possible_truncation)]
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    /// The opening bytes are a marker and a fixed-width name, and the name
    /// stops at its first NUL rather than carrying the padding with it.
    #[test]
    fn the_handshake_is_a_marker_and_a_padded_name() {
        let mut held = vec![0u8];
        held.extend_from_slice(b"Pixel 7");
        held.resize(1 + 64, 0);
        held.extend_from_slice(b"after");
        assert_eq!(take_handshake(&mut held).as_deref(), Some("Pixel 7"));
        assert_eq!(held, b"after");
        // One byte short is not a handshake yet, and nothing is consumed.
        let mut early = vec![0u8; 64];
        assert_eq!(take_handshake(&mut early), None);
        assert_eq!(early.len(), 64);
    }

    /// The codec header names the codec and the size the pictures start at.
    #[test]
    fn the_codec_header_names_the_codec_and_the_size() {
        let mut held = b"h264".to_vec();
        held.extend_from_slice(&576u32.to_be_bytes());
        held.extend_from_slice(&1280u32.to_be_bytes());
        held.extend_from_slice(b"then packets");
        assert_eq!(
            take_codec_meta(&mut held),
            Some(CodecMeta {
                codec: "h264".to_string(),
                width: 576,
                height: 1280,
            })
        );
        assert_eq!(held, b"then packets");
    }

    /// Flags come off the top of the timestamp word, and what is left is the
    /// timestamp.
    #[test]
    fn the_two_flags_come_off_the_timestamp() {
        const CONFIG: u64 = 1 << 63;
        const KEY: u64 = 1 << 62;
        let mut held = packet(CONFIG | 7, b"sps+pps");
        held.extend_from_slice(&packet(KEY | 42, b"idr"));
        held.extend_from_slice(&packet(43, b"slice"));
        let packets = take_packets(&mut held).expect("three whole packets");
        assert_eq!(
            packets,
            vec![
                Packet {
                    config: true,
                    key_frame: false,
                    pts: 7,
                    data: b"sps+pps".to_vec()
                },
                Packet {
                    config: false,
                    key_frame: true,
                    pts: 42,
                    data: b"idr".to_vec()
                },
                Packet {
                    config: false,
                    key_frame: false,
                    pts: 43,
                    data: b"slice".to_vec()
                },
            ]
        );
        assert!(held.is_empty(), "whole packets left a tail behind");
    }

    /// A packet split across two reads is one packet, not a lost one.
    #[test]
    fn a_packet_that_straddles_two_reads_still_arrives() {
        let whole = packet(1, b"0123456789");
        let (first, second) = whole.split_at(14);
        let mut held = first.to_vec();
        assert_eq!(take_packets(&mut held), Ok(Vec::new()));
        assert_eq!(held.len(), 14, "a partial packet must be kept whole");
        held.extend_from_slice(second);
        let packets = take_packets(&mut held).expect("the packet completes");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].data, b"0123456789");
        assert!(held.is_empty());
    }

    /// A length nothing could have sent is a reader that lost its place, and
    /// it says so instead of trusting the number.
    #[test]
    fn an_impossible_length_is_refused_rather_than_believed() {
        let mut held = packet(0, b"");
        held.truncate(8);
        held.extend_from_slice(&(64 * 1024 * 1024u32).to_be_bytes());
        held.extend_from_slice(b"whatever follows");
        let refused = take_packets(&mut held).expect_err("a desync");
        assert_eq!(refused.said, 64 * 1024 * 1024);
        assert!(
            refused.to_string().contains("lost its place"),
            "the refusal does not say what happened: {refused}"
        );
    }
}
