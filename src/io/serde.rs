use std::marker::{Sync, Send};
use std::io::Write;
use std::result::Result;
use rmp;
use rustc_serialize::{Encodable, Decodable};

/// `Encodable`/`Decodable`な型とバイナリ間のシリアライズ・デシリアライズを行う
pub trait SerDe: Send+Sync {
    /// `Encodable`なオブジェクトをシリアライズする
    fn serialize<T: Encodable>(&self, wr: &mut Write, obj: &T) -> Result<(), ()>;

    /// `Encodable`なオブジェクトをシリアライズする
    fn serialize_to_vec<T: Encodable>(&self, obj: &T) -> Option<Vec<u8>> {
        let mut buf: Vec<u8> = Vec::new();
        match self.serialize(&mut buf, obj) {
            Ok(_) => Some(buf),
            _ => None
        }
    }

    /// シリアライズされたバイナリを元に`Decodable`なオブジェクトへデシリアライズする
    fn deserialize<T: Decodable>(&self, binary: &[u8]) -> Option<T>;
}

#[derive(Clone)]
pub struct MsgPackSerDe;

impl SerDe for MsgPackSerDe {
    fn serialize<T: Encodable>(&self, wr: &mut Write, obj: &T) -> Result<(), ()> {
        match obj.encode(&mut rmp::Encoder::new(wr)) {
            Ok(_) => Ok(()),
            _ => Err(()),
        }
    }

    fn deserialize<T: Decodable>(&self, binary: &[u8]) -> Option<T> {
        match Decodable::decode(&mut rmp::Decoder::new(binary)) {
            Ok(x) => Some(x),
            _ => None
        }
    }
}

#[test]
fn test_msgpack_roundtrip() {
    let mp = MsgPackSerDe;
    let bin = match mp.serialize_to_vec(&SerDeTest(1234)) {
        Some(x) => x,
        _ => panic!("err#1"),
    };
    match mp.deserialize::<SerDeTest>(&bin) {
        Some(x) => assert_eq!(x.0, 1234),
        _ => panic!("err#2"),
    }

    // u32とu64はmsgpack上互換があるのでデシリアライズに成功する
    match mp.deserialize::<SerDeTest3>(&bin) {
        Some(x) => assert_eq!(x.0, 1234),
        _ => panic!("err#3"),
    }

    // u8の範囲外なのでエラー
    assert!(mp.deserialize::<SerDeTest2>(&bin).is_none());

    // i32とu32は互換性がない(符号の有無でMsgPackの型が異なる)ためエラー
    assert!(mp.deserialize::<SerDeTest4>(&bin).is_none());

    let bin = match mp.serialize_to_vec(&SerDeTest5::T1(SerDeTest4(-1234))) {
        Some(x) => x,
        _ => panic!("err#4"),
    };
    match mp.deserialize::<SerDeTest5>(&bin) {
        Some(x) => {
            match x {
                SerDeTest5::T1(x) => assert_eq!(x.0, -1234),
                _ => panic!("err#5"),
            }
        },
        _ => panic!("err#6"),
    }
}

#[cfg(test)]
#[derive(RustcDecodable, RustcEncodable)]
struct SerDeTest(u32);

#[cfg(test)]
#[derive(RustcDecodable, RustcEncodable)]
struct SerDeTest2(u8);

#[cfg(test)]
#[derive(RustcDecodable, RustcEncodable)]
struct SerDeTest3(u64);

#[cfg(test)]
#[derive(RustcDecodable, RustcEncodable)]
struct SerDeTest4(i32);

#[cfg(test)]
#[derive(RustcDecodable, RustcEncodable)]
enum SerDeTest5 {
    T0(SerDeTest),
    T1(SerDeTest4),
}
