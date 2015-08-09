//! ネットワークモジュール

use std::any::{Any};
use std::io::Result;
use std::net::SocketAddr;
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
pub trait Serializable: Any+Encodable+Decodable+Clone+Debug+Sync+Send {}
impl<T: Any+Encodable+Decodable+Clone+Debug+Sync+Send> Serializable for T {}

/// リクエスト・レスポンスからなる問合せ用ソケット
pub trait InquirySocket {
    /// リクエストを処理するハンドラを登録
    ///
    /// `id`で型を識別するためエンドポイント間で`id`と型の対応を同じにする必要がる
    fn register_handler<T, TREQ, TRES>(&self, id: u32, handler: T) -> bool
        where TREQ: Serializable, TRES: Serializable, T: 'static+Send+Sync+Fn(TREQ)->TRES;

    /// 指定したリクエストメッセージを利用して `remote_ep` で指定するリモートに対して問合せを行う.
    ///
    /// レスポンスが帰ってきた場合や応答がなかった場合は `callback` が呼び出される．
    fn inquire<TREQ, TRES, CB>(&self, msg: TREQ, remote_ep: &SocketAddr, callback: CB) -> Result<()>
        where TREQ: Serializable, TRES: Serializable, CB: 'static+Fn(TREQ, Option<TRES>)+Send+Sync+Any;
}

/// 再送タイムアウトを求めるアルゴリズム
pub trait RetransmissionTimerAlgorithm<T: Send + Sync>: Sync+Send {
    /// 往復時間情報を追加します
    ///
    /// メッセージ送信時間(相対時刻[ms]), 往復時間([ms]), 再送数を指定して
    /// 往復時間情報を追加します．再送が発生していた場合，メッセージ送信時間および
    /// 往復時間は，最初のメッセージを送信した時間を基準にします．
    fn add_sample(&self, key: &T, time: u64, rtt: u32, retransmit_cnt: u32);

    /// 再送タイムアウト時間[ms]を取得します
    fn get_rto(&self, key: &T, retransmit_count: u32) -> u32;
}
