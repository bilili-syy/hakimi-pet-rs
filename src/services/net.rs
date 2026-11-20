//! HTTP：ureq Agent，带代理。
//! TODO(B)：config.proxy 非空用它；为空则读注册表
//! HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings 的 ProxyEnable/ProxyServer（跟随系统代理，Clash 之类）
#![allow(dead_code)]

pub fn agent(timeout_secs: u64) -> ureq::Agent {
    let _ = timeout_secs;
    ureq::Agent::new_with_defaults()
}
