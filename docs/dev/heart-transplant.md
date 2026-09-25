# Heart Transplant For External Components

Every out-of-tree service and D1 driver must support heart transplant. This is
not a manifest-only opt-in: the process must cooperate with live state transfer,
quiesce safely, validate adopted state, and preserve every capability required
to continue serving after activation.

The protocol is **stage → live bulk sync → catch up → quiesce → validate →
commit → reclaim**. Preparation or precommit failure aborts to the old owner.
The product keeps the old archive authoritative until activation succeeds and
the durable registry commits the archive identity, manifest metadata, content
hash, and accepted generation.

## Native entry point

Decode structured startup and split cold start from adoption:

```rust
let control = Channel(startup_channel);
let startup = Startup::receive(control)
    .unwrap_or_else(|_| bexos_component::exit());

let state = if startup.migration_target {
    bexos_component::live_migration::receive::<ComponentState>(
        control,
        startup.migration_generation,
    )
    .unwrap_or_else(|_| bexos_component::exit())
} else {
    Startup::ready(control).unwrap_or_else(|_| bexos_component::exit());
    ComponentState::new(control, startup.migration)
};

serve(state)
```

The candidate must not repeat cold-start registration or reacquire exclusive
resources already owned by the source. Adoption reconstructs authority from
the transferred state and resources.

## Implement versioned state

`bexos_component::live_migration::State` is the stable native interface:

```rust
use bexos_component::{
    Channel,
    live_migration::{Resource, State},
    migration_codec::{
        Error,
        codec::{Decoder, Encoder},
    },
};

struct ComponentState {
    control: Channel,
    migration: Option<Channel>,
    device: Option<u64>,
    requests: u64,
}

impl State for ComponentState {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            device: None,
            requests: 0,
        }
    }

    fn keys(&self) -> Vec<u64> { vec![0] }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 { return Err(Error::InvalidData); }
        let mut out = Encoder::new();
        out.word(1); // component-owned record version
        out.word(self.control.0);
        out.word(self.migration.map_or(0, |channel| channel.0));
        out.word(self.device.unwrap_or(0));
        out.word(self.requests);
        Ok(Some(out.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>)
        -> Result<(), Error>
    {
        if key != 0 { return Err(Error::InvalidData); }
        let mut input = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if input.word()? != 1 { return Err(Error::UnsupportedVersion); }
        self.control = Channel(input.word()?);
        let migration = input.word()?;
        self.migration = (migration != 0).then_some(Channel(migration));
        let device = input.word()?;
        self.device = (device != 0).then_some(device);
        self.requests = input.word()?;
        input.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let mut resources = vec![Resource::Handle(self.control.0)];
        if let Some(channel) = self.migration {
            resources.push(Resource::Handle(channel.0));
        }
        if let Some(handle) = self.device {
            resources.push(Resource::Handle(handle));
        }
        resources
    }

    fn activated(&mut self, generation: u64) {
        let _ = generation;
        bexos_component::log("component: adopted state version 1\n");
    }
}
```

Version each record independently. Never serialize Rust object layouts,
allocator pointers, vtables, locks, old executable addresses, or architecture-
dependent padding. Split large collections into stable bounded records rather
than one monolith. A record payload is limited to 32,704 bytes, the default
state limit is 8 MiB, and the source accepts at most 16,384 unique keys.

`resources()` declares authority that must move with the state. Use
`Resource::Handle`, `Resource::Mapping`, or `Resource::Pin` as appropriate.
State validation must prove that every required relationship and resource is
present before commit.

## Serve while migration progresses

```rust
pub fn serve(mut state: ComponentState) -> ! {
    let mut migration =
        bexos_component::live_migration::Source::new(state.migration);

    loop {
        if migration.poll(&state).is_err() {
            let _ = bexos_component::migration::abort();
        }
        if !migration.quiescing() {
            state.requests = state.requests.wrapping_add(1);
            migration.changed(0);
            // Dispatch one bounded unit of normal work here.
        }
        bexos_component::yield_now();
    }
}
```

Call `changed(key)` after mutating a record so catch-up resends it. Keep normal
dispatch bounded and poll migration regularly. During quiescence, stop state-
changing work. `quiescence_ready` may remain false while bounded in-flight
device work drains. Use `prepare_adoption` for expensive derived reconstruction
before the frozen cutover and `finish_adoption` for final validation.

If `poll` fails, abort; do not continue toward commit with partial state. The
source remains authoritative on precommit failure. After commit, the new owner
must not assume the old process can recover resources for it.

## What must be preserved

For a service, consider:

- Control, migration, client, provider, and subscription channels.
- Request IDs, queued requests/responses, replay watermarks, and deadlines.
- Service registry identities and retained endpoints.
- In-memory configuration generation and accepted durable state.

For a driver, also consider:

- MMIO/register-proxy mappings and interrupt handles.
- IOMMU domains, DMA mappings, pins/tokens, and ring ownership.
- Device topology identity, lifecycle/recovery endpoints, and published service
  instances.
- Hardware producer/consumer indices and completed-write watermarks so work is
  neither lost nor replayed.

## Replacement archive

Build a replacement from the new source with the same package identity,
component type, executable path, service contract, and compatible state reader.
The replacement should read every state version still eligible for update; it
may emit only its newest version.

```starlark
bexos_service(
    name = "echo_replacement_aarch64",
    srcs = ["src/replacement.rs", "src/state.rs"],
    architecture = "aarch64",
    binary_name = "echo_service",
    manifest = "echo.prototxt",
    signing_key = "signing.key",
    std = True,
)
```

The acceptance fixture builds separate initial and replacement service/driver
archives and proves state plus handle/resource continuity under QEMU. Follow
that shape; a build-only replacement is not evidence of a successful handover.

Portable WASM services use the corresponding `checkpoint`, `restore`,
`activate`, and `abort` lifecycle methods described in [Services](services.md).
For platform-wide protocol details and current limits, see the internal
[heart-transplant guide](../heart-transplant.md).
