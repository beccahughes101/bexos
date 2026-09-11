use fidlc::ast::{Decl, Payload, TypeKind};
use fidlc::capability::split_library;
use fidlc::ir::{Library, WIRE_ABI_VERSION, stable_ordinal};
use fidlc::parser::parse_file;
use fidlc::validate::validate;
use fidlc::{compile_rust_source, compile_rust_sources, compile_rust_sources_with_deps};

const CAMERA_FIDL: &str = r#"
library bexos.hardware.camera;

using bexos.media;

alias FrameBytes = vector<uint8>:4096;

enum CameraState : uint32 {
    IDLE = 0;
    STREAMING = 1;
};

bits CameraFlags : uint32 {
    HDR = 1;
    DEPTH = 2;
};

struct CameraStatus {
    ready bool;
    state CameraState;
};

table CaptureOptions {
    1: width uint32;
    2: height uint32;
};

protocol CameraController {
    1: GetStatus() -> (struct {
        ready bool;
        state CameraState;
    });

    @permission("CAMERA")
    CaptureFrame(struct {
        options CaptureOptions?;
    }) -> (struct {
        frame vector<uint8>;
    });

    @permission("android.permission.CAMERA_RECORD=Camera1")
    RecordStream(resource struct {
        client_sink client_end:AudioVideoSink;
    });

    @permission("request.permissions.contains('CAMERA') && client.is_foreground")
    RecordHighResVideo() -> (struct {
        stream_handle handle;
    });
};
"#;

#[test]
fn parser_accepts_v1_declarations_and_typed_payloads() {
    let file = parse_file(CAMERA_FIDL).expect("parse should succeed");
    assert_eq!(file.library, "bexos.hardware.camera");
    assert_eq!(file.usings[0].library, "bexos.media");
    assert_eq!(file.declarations.len(), 6);

    let protocol = protocol(&file.declarations, "CameraController");
    assert_eq!(protocol.methods.len(), 4);
    assert_eq!(protocol.methods[0].ordinal, Some(1));
    assert_eq!(protocol.methods[1].permission(), Some("CAMERA"));

    let Some(Payload::Struct(response)) = &protocol.methods[1].response else {
        panic!("CaptureFrame should have a struct response");
    };
    assert_eq!(response.fields[0].name, "frame");
    assert!(matches!(
        response.fields[0].ty.kind,
        TypeKind::Vector(_, Some(_)) | TypeKind::Vector(_, None)
    ));
}

#[test]
fn parser_concatenates_permission_string_literals_and_rejects_malformed_syntax() {
    let file = parse_file(
        r#"
library bexos.hardware.camera;
protocol CameraController {
    @permission("CAM" "ERA")
    CaptureFrame();
};
"#,
    )
    .expect("parse should succeed");

    assert_eq!(
        protocol(&file.declarations, "CameraController").methods[0].permission(),
        Some("CAMERA")
    );

    let err = parse_file(
        r#"
library bexos.hardware.camera;
protocol CameraController {
    @permission()
    CaptureFrame();
};
"#,
    )
    .and_then(|file| validate(&file).map(|_| file))
    .expect_err("empty permission should fail validation");
    assert!(err.contains("@permission requires"));
}

#[test]
fn validation_rejects_duplicate_methods_unknown_types_and_invalid_bits() {
    let duplicate = parse_file(
        r#"
library bexos.hardware.camera;
protocol CameraController {
    CaptureFrame();
    CaptureFrame();
};
"#,
    )
    .expect("parse should succeed");
    assert!(
        validate(&duplicate)
            .expect_err("duplicate method should fail")
            .contains("duplicate method")
    );

    let unknown = parse_file(
        r#"
library bexos.hardware.camera;
struct Bad {
    missing NotDeclared;
};
"#,
    )
    .expect("parse should succeed");
    assert!(
        validate(&unknown)
            .expect_err("unknown type should fail")
            .contains("unknown type")
    );

    let bad_bits = parse_file(
        r#"
library bexos.hardware.camera;
bits Flags : uint32 {
    BAD = 3;
};
"#,
    )
    .expect("parse should succeed");
    assert!(
        validate(&bad_bits)
            .expect_err("invalid bits should fail")
            .contains("single bit")
    );
}

#[test]
fn ir_assigns_stable_ordinals_and_capability_groups() {
    let ast = parse_file(CAMERA_FIDL).expect("parse should succeed");
    validate(&ast).expect("validation should succeed");
    let library = Library::from_ast(ast);
    let methods = &library.protocols[0].methods;

    assert_eq!(methods[0].ordinal, 1);
    assert_eq!(
        methods[1].ordinal,
        stable_ordinal("CameraController", "CaptureFrame")
    );

    let groups = split_library(&library);
    assert_eq!(groups.len(), 4);
    assert!(groups.iter().any(|group| {
        group.capability == "Public"
            && group.permission.is_none()
            && group.methods[0].name == "GetStatus"
            && group.methods[0].ordinal == 1
    }));
    assert!(groups.iter().any(|group| {
        group.capability == "Camera" && group.permission.as_deref() == Some("CAMERA")
    }));
}

#[test]
fn rust_generator_is_deterministic_and_emits_v1_api_surface() {
    let first = compile_rust_source(CAMERA_FIDL).expect("generation should succeed");
    let second = compile_rust_source(CAMERA_FIDL).expect("generation should succeed");

    assert_eq!(first, second);
    assert!(first.contains("#![no_std]"));
    assert!(first.contains(&format!(
        "pub const BEXOS_FIDL_WIRE_ABI_VERSION: u32 = {};",
        WIRE_ABI_VERSION
    )));
    assert!(first.contains("pub struct MethodBinding"));
    assert!(first.contains("pub static CAPABILITY_BINDINGS"));
    assert!(first.contains("pub struct CameraControllerCaptureFrameResponse<'a>"));
    assert!(first.contains("pub trait CameraControllerCameraServer"));
    assert!(first.contains("pub struct CameraControllerCameraClient<T>"));
    assert!(first.contains("fn encode(&self, bytes: &mut [u8], handles: &mut [HandleRef])"));
    assert!(first.contains("request.permissions.contains('CAMERA') && client.is_foreground"));
}

#[test]
fn parser_generates_netstack_fidl_surface() {
    let generated = compile_rust_source(
        r#"
library bexos.net;

struct Ipv4Address {
    octets array<uint8, 4>;
};

struct Ipv6Address {
    octets array<uint8, 16>;
};

type IpAddress = strict union {
    1: ipv4 Ipv4Address;
    2: ipv6 Ipv6Address;
};

struct SocketAddress {
    addr IpAddress;
    port uint16;
};

table SocketOptions {
    1: non_blocking bool;
};

protocol TcpSocket {
    1: GetStream() -> (resource struct {
        status int32;
        socket handle:SOCKET;
    });
};

@discoverable
protocol Netstack {
    1: ConnectTcp(resource struct {
        remote_addr SocketAddress;
        options SocketOptions;
        socket server_end:TcpSocket;
    }) -> (struct {
        status int32;
    });
};
"#,
    )
    .expect("generation should succeed");

    assert!(generated.contains("pub enum IpAddress"));
    assert!(generated.contains("pub struct NetstackConnectTcpRequest"));
    assert!(generated.contains("pub trait NetstackPublicServer"));
    assert!(generated.contains("socket: HandleRef"));
}

#[test]
fn system_privileged_permissions_generate_non_public_capability_group() {
    let source = r#"
library bexos.kernel;

protocol SystemPrivileged {
    @permission("BEXOS_SYSTEM_PRIVILEGED")
    CreateProcess();

    @permission("BEXOS_SYSTEM_PRIVILEGED")
    BindInterrupt();
};
"#;
    let ast = parse_file(source).expect("parse should succeed");
    validate(&ast).expect("validation should succeed");
    let library = Library::from_ast(ast);
    let groups = split_library(&library);

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].protocol, "SystemPrivileged");
    assert_eq!(groups[0].capability, "BexosSystemPrivileged");
    assert_eq!(
        groups[0].permission.as_deref(),
        Some("BEXOS_SYSTEM_PRIVILEGED")
    );
    assert!(!groups.iter().any(|group| group.capability == "Public"));

    let generated = compile_rust_source(source).expect("generation should succeed");
    assert!(generated.contains("pub trait SystemPrivilegedBexosSystemPrivilegedServer"));
    assert!(generated.contains("pub struct SystemPrivilegedBexosSystemPrivilegedClient<T>"));
}

#[test]
fn generated_abi_helpers_use_little_endian_offsets_and_handle_indices() {
    let generated = compile_rust_source(CAMERA_FIDL).expect("generation should succeed");

    assert!(generated.contains("value.to_le_bytes()"));
    assert!(generated.contains("fn align8(value: usize) -> usize"));
    assert!(generated.contains("put_u64(bytes"));
    assert!(generated.contains("get_u64(bytes"));
    assert!(generated.contains("handles[handle_offset + handle_count]"));
    assert!(generated.contains("put_u32(bytes"));
}

#[test]
fn parser_generates_storage_block_fidl_surface() {
    let generated = compile_rust_source(
        r#"
library bexos.storage.block;

using bexos.kernel;

type Status = strict enum : int32 {
    OK = 0,
    ERR_INVALID_HANDLE = -1,
    ERR_INVALID_ARGS = -8,
};

type BlockFlags = strict bits : uint32 {
    READ_ONLY = 0x00000001,
    REMOVABLE = 0x00000002,
    TRIM_SUPPORTED = 0x00000004,
};

type BlockOpcode = strict enum : uint8 {
    READ = 1,
    WRITE = 2,
    FLUSH = 3,
    TRIM = 4,
};

struct BlockInfo {
    block_size uint32;
    block_count uint64;
    max_transfer_blocks uint32;
    flags BlockFlags;
};

struct BlockRequest {
    req_id uint64;
    opcode BlockOpcode;
    vmo_id uint32;
    vmo_offset_blocks uint64;
    device_block_offset uint64;
    block_count uint32;
};

struct BlockResponse {
    req_id uint64;
    status Status;
};

protocol BlockDevice {
    1: GetInfo() -> (struct { info BlockInfo; });
    2: RegisterBuffer(resource struct { vmo handle:VMO; }) -> (struct {
        status Status;
        vmo_id uint32;
    });
    3: UnregisterBuffer(struct { vmo_id uint32; }) -> (struct { status Status; });
    4: GetFifo() -> (resource struct {
        status Status;
        fifo_handle handle:CHANNEL;
    });
};
"#,
    )
    .expect("block fidl should generate");

    assert!(generated.contains("pub const FIDL_LIBRARY: &str = \"bexos.storage.block\";"));
    assert!(generated.contains("pub struct BlockInfo"));
    assert!(generated.contains("pub enum BlockOpcode"));
    assert!(generated.contains("pub trait BlockDevicePublicServer"));
    assert!(generated.contains("pub struct BlockDevicePublicClient<T>"));
    assert!(generated.contains("pub static CAPABILITY_BINDINGS"));
}

#[test]
fn parser_accepts_kernel_design_dialect_across_multiple_files() {
    let generated = compile_rust_sources(&[
        r#"
library bexos.kernel;
type Rights = strict bits : uint32 {
    READ = 0x00000002,
    WRITE = 0x00000004,
    EXECUTE = 0x00000008,
    ADMIN = 0x80000000,
};
type Signals = strict bits : uint32 {
    READABLE = 0x00000001,
};
type Status = strict enum : int32 {
    OK = 0,
    ERR_INVALID_HANDLE = -1,
};
"#
        .to_string(),
        r#"
library bexos.kernel;
protocol ChannelControl {
    ReadMessage(resource struct {
        channel handle:CHANNEL,
        max_bytes uint32,
        max_handles uint32
    }) -> (resource struct {
        status Status,
        data vector<uint8>,
        handles vector<handle>
    });
};
"#
        .to_string(),
        r#"
library bexos.kernel;
protocol TaskControl {
    WaitMany(resource struct {
        items vector<struct {
            h handle,
            signals Signals
        }>,
        deadline_nanos int64
    }) -> (struct {
        status Status,
        satisfied_index uint32,
        observed_signals Signals
    });
};
"#
        .to_string(),
    ])
    .expect("kernel design dialect should generate");

    assert!(generated.contains("pub struct Rights(pub u32);"));
    assert!(generated.contains("pub enum Status"));
    assert!(generated.contains("ErrInvalidHandle = -1"));
    assert!(generated.contains("pub struct InlineVectorStruct1"));
    assert!(generated.contains(
        "handles[handle_offset + handle_count..handle_offset + handle_count + self.handles.len()]"
    ));
    assert!(generated.contains("value.encode_with_handle_offset("));
    assert!(generated.contains("WireVector::from_wire("));
    assert!(generated.contains(", handles, count)?"));
    assert!(generated.contains("pub trait ChannelControlPublicServer"));
    assert!(generated.contains("pub trait TaskControlPublicServer"));
}

#[test]
fn protocol_composition_flattens_parent_methods_and_preserves_one_way_calls() {
    let generated = compile_rust_source(
        r#"
library bexos.fs;
protocol Node {
    1: GetAttr() -> (struct { size uint64; });
    2: Close();
};
protocol File {
    compose Node;
    10: Read(struct { count uint64; }) -> (struct { data vector<uint8>; });
};
"#,
    )
    .expect("composed protocol should generate");

    assert!(generated.contains("pub trait FilePublicServer"));
    assert!(generated.contains("fn get_attr<'a>"));
    assert!(generated.contains(
        "fn close<'a>(&mut self, request: NodeCloseRequest) -> Result<(), FidlWireError>;"
    ));
    assert!(generated.contains("self.transport.send(2"));
    assert_eq!(
        generated.matches("pub struct NodeGetAttrRequest").count(),
        1
    );
}

#[test]
fn protocol_composition_rejects_cycles() {
    let error = compile_rust_source(
        r#"
library bexos.fs;
protocol A { compose B; };
protocol B { compose A; };
"#,
    )
    .expect_err("composition cycle must fail");
    assert!(error.contains("composition cycle"));
}

#[test]
fn parser_and_validator_accept_strict_unions() {
    let file = parse_file(
        r#"
library bexos.kernel;
struct FairProfile {
    priority uint8;
};
struct DeadlineProfile {
    period_ns uint64;
};
type SchedulingProfileInfo = strict union {
    1: fair FairProfile;
    2: deadline DeadlineProfile;
};
"#,
    )
    .expect("parse should succeed");
    validate(&file).expect("strict union should validate");

    let Some(Decl::Union(union)) = file
        .declarations
        .iter()
        .find(|decl| decl.name() == "SchedulingProfileInfo")
    else {
        panic!("union declaration should exist");
    };
    assert!(union.strict);
    assert_eq!(union.members[0].ordinal, 1);
    assert_eq!(union.members[1].name, "deadline");
}

#[test]
fn validator_rejects_bad_union_ordinals() {
    let duplicate = parse_file(
        r#"
library bexos.kernel;
struct Payload { value uint64; };
type Bad = strict union {
    1: first Payload;
    1: second Payload;
};
"#,
    )
    .expect("parse should succeed");
    assert!(
        validate(&duplicate)
            .expect_err("duplicate ordinal should fail")
            .contains("duplicate member ordinal")
    );

    let zero = parse_file(
        r#"
library bexos.kernel;
struct Payload { value uint64; };
type Bad = strict union {
    0: missing Payload;
};
"#,
    )
    .expect("parse should succeed");
    assert!(
        validate(&zero)
            .expect_err("zero ordinal should fail")
            .contains("zero member ordinal")
    );
}

#[test]
fn rust_generator_emits_union_string_vector_and_array_codecs() {
    let generated = compile_rust_source(
        r#"
library bexos.kernel;
type Mode = strict enum : uint8 {
    USER = 1,
    KERNEL = 2,
};
struct FairProfile {
    priority uint8;
};
type SchedulingProfileInfo = strict union {
    1: fair FairProfile;
};
struct ShapeCoverage {
    names vector<string:32>:8;
    uuid array<uint8, 16>;
    modes array<Mode, 2>;
    profiles vector<SchedulingProfileInfo>:8;
};
"#,
    )
    .expect("generation should succeed");

    assert!(generated.contains("pub enum SchedulingProfileInfo"));
    assert!(generated.contains("Fair(FairProfile)"));
    assert!(generated.contains("FairProfile::decode(payload, handles)?"));
    assert!(generated.contains("FidlWireError::UnknownOrdinal(ordinal)"));
    assert!(generated.contains("pub struct WireStringVector<'a>"));
    assert!(generated.contains("pub names: WireStringVector<'a>"));
    assert!(generated.contains("names.encode_wire"));
    assert!(generated.contains("pub profiles: WireVector<'a, SchedulingProfileInfo>"));
    assert!(generated.contains("profiles.encode_wire"));
    assert!(generated.contains("pub uuid: [u8; 16]"));
    assert!(generated.contains("pub modes: [Mode; 2]"));
    assert!(generated.contains("put_u8(bytes, 16, self.uuid[0])"));
    assert!(generated.contains("Mode::decode_value(get_u8(bytes"));
}

#[test]
fn rust_generator_uses_dependency_crate_for_qualified_imported_types() {
    let root = r#"
library bexos.tracing;

using bexos.kernel;

protocol TraceController {
    1: Start() -> (struct { status bexos.kernel.Status; });
};
"#
    .to_string();
    let kernel_dep = r#"
library bexos.kernel;

enum Status : int32 {
    OK = 0;
    ERR_INVALID_ARGS = -8;
};
"#
    .to_string();

    let generated = compile_rust_sources_with_deps(&[root], &[kernel_dep])
        .expect("qualified imported dependency type should generate");

    assert!(generated.contains("pub status: kernel_fidl::Status"));
    assert!(generated.contains("self.status as i32"));
    assert!(generated.contains("kernel_fidl::Status::decode_value"));
}

#[test]
fn validator_rejects_qualified_imported_type_without_dependency_ast() {
    let error = compile_rust_sources_with_deps(
        &[r#"
library bexos.tracing;

using bexos.kernel;

protocol TraceController {
    1: Start() -> (struct { status bexos.kernel.Status; });
};
"#
        .to_string()],
        &[],
    )
    .expect_err("qualified external type should require a dependency AST");

    assert!(error.contains("unknown imported type `bexos.kernel.Status`"));
}

fn protocol<'a>(decls: &'a [Decl], name: &str) -> &'a fidlc::ast::Protocol {
    decls
        .iter()
        .find_map(|decl| match decl {
            Decl::Protocol(protocol) if protocol.name == name => Some(protocol),
            _ => None,
        })
        .expect("protocol should exist")
}
