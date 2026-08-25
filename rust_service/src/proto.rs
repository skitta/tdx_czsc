// 生成的 proptobuf 消息里 RequestEnvelope/ResponseEnvelope 的 body 联合体
// 存在大体积变体，属 protoc 生成代码预期形态，不做改动。
#![allow(clippy::large_enum_variant)]

pub mod analysis {
    include!(concat!(env!("OUT_DIR"), "/tdx.czsc.v1.rs"));
}
