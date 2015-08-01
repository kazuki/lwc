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

mod rfc6298;
pub use self::rfc6298::{RFC6298BasedRTO};

pub mod overlay;

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

/// 再送タイムアウトを求めるアルゴリズム
pub trait RetransmissionTimerAlgorithm<T> {
    /// 往復時間情報を追加します
    ///
    /// メッセージ送信時間(相対時刻[ms]), 往復時間([ms]), 再送数を指定して
    /// 往復時間情報を追加します．再送が発生していた場合，メッセージ送信時間および
    /// 往復時間は，最初のメッセージを送信した時間を基準にします．
    fn add_sample(&self, key: &T, time: u64, rtt: u32, retransmit_cnt: u32);

    /// 再送タイムアウト時間[ms]を取得します
    fn get_rto(&self, key: &T) -> u32;
}
