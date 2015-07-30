use std::marker::{Sync, Send};
use rmp;
use rustc_serialize::{Encodable, Decodable};

/// `Encodable`/`Decodable`な型とバイナリ間のシリアライズ・デシリアライズを行う
pub trait SerDe: Send+Sync {
    /// `Encodable`なオブジェクトをシリアライズする
    fn serialize<T: Encodable>(&self, obj: &T) -> Option<Vec<u8>>;

    /// シリアライズされたバイナリを元に`Decodable`なオブジェクトへデシリアライズする
    fn deserialize<T: Decodable>(&self, binary: &[u8]) -> Option<T>;
}

#[derive(Clone)]
pub struct MsgPackSerDe;

impl SerDe for MsgPackSerDe {
    fn serialize<T: Encodable>(&self, obj: &T) -> Option<Vec<u8>> {
        let mut buf: Vec<u8> = Vec::new();
        match obj.encode(&mut rmp::Encoder::new(&mut buf)) {
            Ok(_) => Some(buf),
            _ => None
        }        
    }

    fn deserialize<T: Decodable>(&self, binary: &[u8]) -> Option<T> {
        match Decodable::decode(&mut rmp::Decoder::new(binary)) {
            Ok(x) => Some(x),
            _ => None
        }
    }
}
