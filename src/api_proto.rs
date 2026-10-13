//! Codec helpers for iTerm2's Python API protocol (vendored `proto/api.proto`, `S:1436-1447`,
//! `S:726-730`, `S:1040-1044`). `api_pb` is generated once by `tools/protogen` and checked in;
//! this module wraps it with the framing iTerm2 uses over the WebSocket.

pub use crate::api_pb as pb;

use prost::Message;
use sha2::{Digest, Sha256};

/// iTerm2's protocol version, sent as `X-iTerm2-Protocol-Version` on a successful upgrade
/// (`W:520-547`).
pub const PROTOCOL_VERSION: &str = "1.19";

/// The WebSocket subprotocol iTerm2 (and Termurion) speak.
pub const SUBPROTOCOL: &str = "api.iterm2.com";

/// iTerm2's text for a request whose oneof it could not parse (`S:1040-1044`), "iTerm2" replaced
/// by "Termurion".
pub const MALFORMED: &str = "Invalid request. Upgrade Termurion to a newer version.";

/// Lowercase hex SHA-256 of the vendored `proto/api.proto`. `proto_is_vendored_unchanged` checks
/// this against the file itself, so a forgotten regeneration after an edit fails the test suite.
pub const PROTO_SHA256: &str = "1a946faa4ab5ae67ef6260a035aa0a35705f3e66d44ed018b9ea517e4ab5f4e8";

/// Lowercase hex SHA-256 of `bytes`.
pub fn proto_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Decodes a frame's payload. iTerm2 parses only binary WebSocket frames and silently drops one
/// it cannot decode, with no reply (`S:1436-1447`).
pub fn decode(bytes: &[u8]) -> Option<pb::ClientOriginatedMessage> {
    pb::ClientOriginatedMessage::decode(bytes).ok()
}

/// Encodes a response for the wire.
pub fn encode(m: &pb::ServerOriginatedMessage) -> Vec<u8> {
    m.encode_to_vec()
}

/// A bare reply carrying the request's id and no submessage.
pub fn reply_to(req: &pb::ClientOriginatedMessage) -> pb::ServerOriginatedMessage {
    pb::ServerOriginatedMessage { id: req.id, submessage: None }
}

/// iTerm2's answer to a request whose oneof it could not parse (`S:1040-1044`).
pub fn malformed(req: &pb::ClientOriginatedMessage) -> pb::ServerOriginatedMessage {
    pb::ServerOriginatedMessage {
        id: req.id,
        submessage: Some(pb::server_originated_message::Submessage::Error(MALFORMED.to_string())),
    }
}

/// The one response iTerm2 sends unprompted, with no id (`S:101-102`).
pub fn notification(n: pb::Notification) -> pb::ServerOriginatedMessage {
    pb::ServerOriginatedMessage {
        id: None,
        submessage: Some(pb::server_originated_message::Submessage::Notification(n)),
    }
}

/// The proto message name of a request (as declared in `proto/api.proto`, not prost's
/// Rust-cased variant name), or "unset" when the oneof is empty.
pub fn request_name(req: &pb::ClientOriginatedMessage) -> &'static str {
    use pb::client_originated_message::Submessage as S;
    match &req.submessage {
        None => "unset",
        Some(S::GetBufferRequest(_)) => "GetBufferRequest",
        Some(S::GetPromptRequest(_)) => "GetPromptRequest",
        Some(S::TransactionRequest(_)) => "TransactionRequest",
        Some(S::NotificationRequest(_)) => "NotificationRequest",
        Some(S::RegisterToolRequest(_)) => "RegisterToolRequest",
        Some(S::SetProfilePropertyRequest(_)) => "SetProfilePropertyRequest",
        Some(S::ListSessionsRequest(_)) => "ListSessionsRequest",
        Some(S::SendTextRequest(_)) => "SendTextRequest",
        Some(S::CreateTabRequest(_)) => "CreateTabRequest",
        Some(S::SplitPaneRequest(_)) => "SplitPaneRequest",
        Some(S::GetProfilePropertyRequest(_)) => "GetProfilePropertyRequest",
        Some(S::SetPropertyRequest(_)) => "SetPropertyRequest",
        Some(S::GetPropertyRequest(_)) => "GetPropertyRequest",
        Some(S::InjectRequest(_)) => "InjectRequest",
        Some(S::ActivateRequest(_)) => "ActivateRequest",
        Some(S::VariableRequest(_)) => "VariableRequest",
        Some(S::SavedArrangementRequest(_)) => "SavedArrangementRequest",
        Some(S::FocusRequest(_)) => "FocusRequest",
        Some(S::ListProfilesRequest(_)) => "ListProfilesRequest",
        Some(S::ServerOriginatedRpcResultRequest(_)) => "ServerOriginatedRPCResultRequest",
        Some(S::RestartSessionRequest(_)) => "RestartSessionRequest",
        Some(S::MenuItemRequest(_)) => "MenuItemRequest",
        Some(S::SetTabLayoutRequest(_)) => "SetTabLayoutRequest",
        Some(S::GetBroadcastDomainsRequest(_)) => "GetBroadcastDomainsRequest",
        Some(S::TmuxRequest(_)) => "TmuxRequest",
        Some(S::ReorderTabsRequest(_)) => "ReorderTabsRequest",
        Some(S::PreferencesRequest(_)) => "PreferencesRequest",
        Some(S::ColorPresetRequest(_)) => "ColorPresetRequest",
        Some(S::SelectionRequest(_)) => "SelectionRequest",
        Some(S::StatusBarComponentRequest(_)) => "StatusBarComponentRequest",
        Some(S::SetBroadcastDomainsRequest(_)) => "SetBroadcastDomainsRequest",
        Some(S::CloseRequest(_)) => "CloseRequest",
        Some(S::InvokeFunctionRequest(_)) => "InvokeFunctionRequest",
        Some(S::ListPromptsRequest(_)) => "ListPromptsRequest",
        Some(S::ScreenshotRequest(_)) => "ScreenshotRequest",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::client_originated_message::Submessage as Req;
    use pb::notification_response;
    use prost::Message;

    fn req_with_id(id: i64) -> pb::ClientOriginatedMessage {
        pb::ClientOriginatedMessage { id: Some(id), submessage: None }
    }

    #[test]
    fn wire_bytes_follow_python() {
        let m = decode(&[0x08, 0x01, 0xd2, 0x06, 0x00]).unwrap();
        assert_eq!(m.id, Some(1));
        assert!(matches!(m.submessage, Some(Req::ListSessionsRequest(_))));
        assert_eq!(request_name(&m), "ListSessionsRequest");

        let m = decode(&[0x08, 0x02, 0xaa, 0x07, 0x00]).unwrap();
        assert_eq!(m.id, Some(2));
        assert!(matches!(m.submessage, Some(Req::FocusRequest(_))));

        let m = decode(&[0x08, 0x03, 0xda, 0x07, 0x00]).unwrap();
        assert_eq!(m.id, Some(3));
        assert!(matches!(m.submessage, Some(Req::GetBroadcastDomainsRequest(_))));

        assert!(decode(&[0xff]).is_none());

        let reply = malformed(&req_with_id(7));
        let mut expected = vec![0x08, 0x07, 0x12, MALFORMED.len() as u8];
        expected.extend_from_slice(MALFORMED.as_bytes());
        assert_eq!(encode(&reply), expected);

        let n = notification(pb::Notification::default());
        let bytes = encode(&n);
        assert_ne!(bytes.first(), Some(&0x08));
        assert_eq!(&bytes[bytes.len() - 3..], &[0xc2, 0x3e, 0x00]);

        assert_eq!(request_name(&pb::ClientOriginatedMessage { id: None, submessage: None }), "unset");
    }

    #[test]
    fn reply_to_carries_only_the_id() {
        let r = reply_to(&pb::ClientOriginatedMessage { id: Some(9), submessage: Some(Req::FocusRequest(pb::FocusRequest {})) });
        assert_eq!(r, pb::ServerOriginatedMessage { id: Some(9), submessage: None });
        assert_eq!(reply_to(&pb::ClientOriginatedMessage { id: None, submessage: None }).id, None);
        assert_eq!(encode(&reply_to(&req_with_id(9))), [0x08, 0x09]);
    }

    #[test]
    fn defaults_are_explicit() {
        use pb::transaction_response::Status;
        let r = pb::TransactionResponse { status: Some(Status::Ok as i32) };
        assert_eq!(r.encode_to_vec(), vec![0x08, 0x00]);

        assert_eq!(notification_response::Status::AlreadySubscribed as i32, 4);
    }

    #[test]
    fn proto_is_vendored_unchanged() {
        assert_eq!(proto_sha256(include_bytes!("../proto/api.proto")), PROTO_SHA256);
    }
}
