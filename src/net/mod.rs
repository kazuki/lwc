//! ネットワークモジュール

use std::io::Result;
use std::net::ToSocketAddrs;
use std::fmt::Debug;
use std::marker::{Sync, Send};
use rustc_serialize::{Encodable, Decodable};

mod sock_suppl;
pub use self::sock_suppl::{SocketEventType, DatagramSocket};

mod inquiry_sock;
pub use self::inquiry_sock::{SimpleInquirySocket};

/// シリアライズ可能なことを表す
pub trait Serializable: Encodable+Decodable+Clone+Debug+Sync+Send {}
impl<T: Encodable+Decodable+Clone+Debug+Sync+Send> Serializable for T {}

/// リクエスト・レスポンスからなる問合せ用ソケット
pub trait InquirySocket<TREQ: Serializable, TRES: Serializable> {
    /// 指定したリクエストメッセージを利用して `remote_ep` で指定するリモートに対して問合せを行う.
    ///
    /// レスポンスが帰ってきた場合や応答がなかった場合は `callback` が呼び出される．
    fn inquire<EP, CB>(&mut self, msg: TREQ, remote_ep: EP, callback: CB) -> Result<()>
        where EP: ToSocketAddrs+Clone+Send+Sync+Debug, CB: Fn(Option<TRES>)+Send+Sync;
}

pub mod overlay;
