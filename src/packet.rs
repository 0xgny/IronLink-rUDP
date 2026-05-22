use std::convert::TryInto;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum PacketType {
    Data = 0x01,
    Ack = 0x02,
    Unknown,
}


impl From<u8> for PacketType {
    fn from(val: u8) -> Self {
        match val {
            0x01 => PacketType::Data,
            0x02 => PacketType::Ack,
            _ => PacketType::Unknown,
        }
    }
}

pub struct Packet {
    pub seq_num: u32,
    pub p_type: PacketType,
    pub payload: Vec<u8>,
}

impl Packet {
    // Serialize to bytes: [SeqNum (4)] [Type (1)] [Payload (Variable)]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.seq_num.to_be_bytes());
        bytes.push(self.p_type as u8);
        bytes.extend_from_slice(&self.payload);
        bytes
    }

    // Deserialize from wire bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 5 {
            return None; // Header too short
        }
        let seq_num = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
        let p_type = PacketType::from(bytes[4]);
        let payload = bytes[5..].to_vec();

        Some(Packet { seq_num, p_type, payload })
    }
}
