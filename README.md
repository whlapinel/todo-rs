# Todo — Smithy-rs Server Code Generation Guide

This project is a todo list API written in Rust, using [Smithy IDL](https://smithy.io) to define the service model and [smithy-rs](https://github.com/smithy-lang/smithy-rs) to generate the Rust server SDK from it.

This README explains the full setup from scratch, including what every piece is and why it exists — written for someone comfortable with Rust who has never touched Java, Gradle, or Kotlin.

---

## Quick Start

**Prerequisites:** JDK 11+, Rust 1.91.1+

```sh
# Clone with submodules (smithy-rs is a submodule)
git clone --recurse-submodules <repo-url>
cd todo

# Generate the Rust server SDK from the Smithy model (slow on first run)
./gradlew :model:assemble

# Build and run the server
cargo run
```

If you already cloned without `--recurse-submodules`:

```sh
git submodule update --init --recursive
```

With [Taskfile](https://taskfile.dev) installed, the above is just:

```sh
task codegen
task run
```

---

## What Is Smithy?

Smithy is an interface definition language (IDL) from AWS. You describe your API — its operations, inputs, outputs, errors, and resource hierarchy — in `.smithy` files. Tooling then generates code from that description: server stubs, client SDKs, documentation, etc.

It is similar in spirit to Protobuf or OpenAPI, but more expressive about service semantics (resources, lifecycles, constraints).

---

## What Is smithy-rs?

[smithy-rs](https://github.com/smithy-lang/smithy-rs) is a code generator maintained by AWS that reads Smithy models and outputs Rust code. It generates:

- Typed input/output structs for every operation
- Protocol serialization/deserialization (JSON, XML, CBOR)
- A service builder that wires handlers to HTTP routes
- All required runtime crates

The generated code depends on a set of `aws-smithy-*` runtime crates that live inside the smithy-rs repository itself under `rust-runtime/`. These are referenced via Cargo path dependencies — they are **not** published to crates.io.

---

## Why Is There Java/Gradle Involved?

The smithy-rs code generator is written in **Kotlin** and runs on the JVM. Gradle is the build tool that compiles and runs it. You don't need to write any Kotlin or Java yourself — you just configure a few files and run one command. But you do need:

- A JDK (Java 11+)
- Gradle (provided as a wrapper script — no separate install needed)

Think of Gradle like a Makefile that also handles downloading and compiling dependencies. The Gradle wrapper (`gradlew`) is a shell script checked into the repo that downloads the right Gradle version automatically on first run.

---

## Project Structure

```
todo/
├── smithy-rs/                     # Git submodule — the smithy-rs monorepo
│   └── rust-runtime/              # Runtime crates used by generated code
├── model/                         # Smithy model + Gradle build for codegen
│   ├── src/main/smithy/           # Your .smithy source files
│   ├── smithy-build.json          # Codegen configuration
│   └── build.gradle.kts           # Gradle build for the model subproject
├── todo-server-sdk/               # Generated Rust crate (do not edit)
├── src/
│   └── main.rs                    # Your server implementation
├── Cargo.toml                     # Your Rust package
├── build.gradle.kts               # Root Gradle build (minimal)
├── settings.gradle.kts            # Gradle project settings
├── gradle.properties              # Gradle properties / version pins
├── gradlew / gradlew.bat          # Gradle wrapper scripts
└── gradle/wrapper/                # Gradle wrapper configuration
```

---

## Prerequisites

| Tool | Required Version | How to Check |
|------|-----------------|--------------|
| JDK  | 11 or newer     | `java -version` |
| Rust | 1.91.1 or newer | `rustc --version` |
| Cargo | (comes with Rust) | `cargo --version` |

You do **not** need to install Gradle separately — the wrapper handles it.

---

## How Code Generation Works

### Step 1 — The Smithy Model

Your `.smithy` files in `model/src/main/smithy/` define the API. Here is the service definition:

```smithy
// model/src/main/smithy/service.smithy
$version: "2"
namespace common

use aws.protocols#restJson1

@restJson1
service Listeria {
    version: "2026-04-14"
    resources: [List]
}
```

`@restJson1` tells smithy-rs which wire protocol to generate. The supported protocols are:

- `aws.protocols#restJson1` — REST endpoints with JSON bodies (most common)
- `aws.protocols#restXml` — REST with XML
- `smithy.protocols#rpcv2Cbor` — RPC with CBOR encoding

### Step 2 — smithy-build.json

`model/smithy-build.json` tells the Smithy build tool which code generator plugins to run and how to configure them:

```json
{
  "version": "1.0",
  "projections": {
    "todo-server-sdk": {
      "plugins": {
        "rust-server-codegen": {
          "runtimeConfig": {
            "relativePath": "../smithy-rs/rust-runtime"
          },
          "codegen": {},
          "service": "common#Listeria",
          "module": "todo-server-sdk",
          "moduleVersion": "0.1.0",
          "moduleDescription": "Rust server SDK for todo app",
          "moduleAuthors": ["whlapinel@gmail.com"]
        }
      }
    }
  }
}
```

Key fields:

- `projections` — named build outputs. Each projection runs independently and produces its own set of artifacts. Here we have one projection called `todo-server-sdk`.
- `rust-server-codegen` — the plugin name. This string must match the plugin registered in the smithy-rs Kotlin code.
- `runtimeConfig.relativePath` — tells the codegen where to find the Rust runtime crates. Points to the `rust-runtime/` directory inside the smithy-rs submodule.
- `service` — the fully-qualified Smithy shape ID of your service (`namespace#ShapeName`).
- `module` — the Rust crate name for the generated SDK.

### Step 3 — The Gradle Build

This is where the JVM toolchain lives. There are three Gradle files in the project root and one in `model/`.

#### `settings.gradle.kts` — project structure

```kotlin
rootProject.name = "todo"

includeBuild("smithy-rs")   // composite build — uses smithy-rs source directly

include(":model")           // the model/ directory is a Gradle subproject
```

`includeBuild("smithy-rs")` is the key line. It tells Gradle to treat the `smithy-rs/` submodule as part of this build. When `model/build.gradle.kts` declares a dependency on `software.amazon.smithy.rust:codegen-server:0.1.16`, Gradle resolves it directly from the smithy-rs source code — building the Kotlin codegen on demand — instead of looking for it on Maven Central (where it isn't published).

This is called a **Gradle composite build**. It is how smithy-rs is designed to be used from external projects.

#### `build.gradle.kts` — root build (minimal)

```kotlin
plugins {
    id("base")
}
subprojects {
    repositories {
        mavenCentral()
    }
}
```

Nothing interesting here. Just sets Maven Central as the repository for downloading third-party JVM dependencies (the Smithy libraries and Smithy Gradle plugin).

#### `gradle.properties` — version pins and settings

```properties
version=0.1.0
org.gradle.parallel=true
org.gradle.jvmargs=-Xmx4G
serverSdkCrateName=todo-server-sdk
```

`serverSdkCrateName` is read by the copy task in `model/build.gradle.kts` to know where to put the generated crate.

#### `model/build.gradle.kts` — the codegen build

```kotlin
plugins {
    java
    id("software.amazon.smithy.gradle.smithy-base") version "1.3.0"
}

repositories {
    mavenCentral()
}

dependencies {
    "smithyBuild"("software.amazon.smithy.rust:codegen-server:0.1.16")
    "smithyBuild"("software.amazon.smithy:smithy-aws-traits:1.67.0")
    "smithyBuild"("software.amazon.smithy:smithy-model:1.67.0")
}

tasks {
    val srcDir = projectDir.resolve("../")
    val serverSdkCrateName: String by project
    register<Copy>("copyServerCrate") {
        dependsOn("smithyBuild")
        from(layout.buildDirectory.dir("smithyprojections/model/todo-server-sdk/rust-server-codegen"))
        into("$srcDir/$serverSdkCrateName")
    }
    named("assemble") {
        dependsOn("smithyBuild")
        finalizedBy("copyServerCrate")
    }
}
```

Breaking this down:

- `java` plugin — required because the Smithy Gradle plugin creates per-source-set configurations (like `smithyBuild`) that attach to Java source sets. Without it, the `smithyBuild` configuration doesn't exist.
- `software.amazon.smithy.gradle.smithy-base` version `1.3.0` — the Smithy Gradle plugin. It adds the `smithyBuild` task and configuration, looks for Smithy files in `src/main/smithy/`, and runs `smithy build` using `smithy-build.json`.
- `"smithyBuild"(...)` — the `smithyBuild` configuration is for build-time-only dependencies that are not included in output JARs. This is where you put code generator plugins.
  - `codegen-server:0.1.16` — the Rust server code generator. Gradle resolves this from the smithy-rs submodule via composite build substitution.
  - `smithy-aws-traits` — needed because our model uses `@restJson1` from the `aws.protocols` namespace.
  - `smithy-model` — the core Smithy model library.
- `copyServerCrate` — after the smithy build runs, the generated Rust code lands in `model/build/smithyprojections/model/todo-server-sdk/rust-server-codegen/`. This task copies it to `todo-server-sdk/` at the project root where Cargo can find it.

### Step 4 — Running the Build

```sh
./gradlew :model:assemble
```

**What happens on first run (takes several minutes):**

1. Gradle downloads itself (Gradle 8.14.3)
2. Gradle compiles the smithy-rs buildSrc (Kotlin build conventions)
3. Gradle compiles the smithy-rs codegen Kotlin source: `:codegen-traits`, `:codegen-core`, `:codegen-server`
4. The Smithy Gradle plugin runs `smithy build` on your `.smithy` files
5. The `rust-server-codegen` plugin generates Rust source files
6. The `copyServerCrate` task copies the output to `todo-server-sdk/`

**On subsequent runs (seconds):**

- Gradle caches compiled Kotlin artifacts
- Only re-runs if your `.smithy` files changed

### Step 5 — Using the Generated SDK in Rust

The generated `todo-server-sdk` crate exposes:

- `todo_server_sdk::input::*` — typed input structs for each operation
- `todo_server_sdk::output::*` — typed output structs
- `todo_server_sdk::error::*` — typed error enums
- `todo_server_sdk::Listeria` — the service type (implements `tower::Service`)
- `todo_server_sdk::ListeriaBuilder` — builder that takes your handler functions

Wire it together in `src/main.rs`:

```rust
use todo_server_sdk::{error, input, output, Listeria, ListeriaConfig};

async fn get_list(
    input: input::GetListInput,
) -> Result<output::GetListOutput, error::GetListError> {
    // implement here
    todo!()
}

#[tokio::main]
async fn main() {
    let config = ListeriaConfig::builder().build();
    let app = Listeria::builder(config)
        .get_list(get_list)
        // .list_lists(list_lists)
        // .get_item(get_item)
        // .list_items(list_items)
        .build()
        .unwrap();

    hyper::Server::bind(&"127.0.0.1:3000".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}
```

The handler signature is always:

```rust
async fn handler_name(input: input::OperationNameInput) -> Result<output::OperationNameOutput, error::OperationNameError>
```

The framework handles HTTP parsing, routing, deserialization of the request, and serialization of the response. You only implement business logic.

---

## The Smithy Model

### Resource Hierarchy

The model follows a `User → List → Item` hierarchy:

```
User (userId)
└── List (userId, listId)
    └── Item (userId, listId, itemId)
```

Child resources inherit parent identifiers. An `Item` requires all three IDs because:

- `itemId` alone is not globally unique
- An item always belongs to a specific list belonging to a specific user

### Operations and HTTP Bindings

| Operation | HTTP | Path |
|-----------|------|------|
| `GetList` | GET | `/users/{userId}/lists/{listId}` |
| `ListLists` | GET | `/users/{userId}/lists` |
| `GetItem` | GET | `/users/{userId}/lists/{listId}/items/{itemId}` |
| `ListItems` | GET | `/users/{userId}/lists/{listId}/items` |

Path parameters are marked with `@httpLabel` in the input shape. The generated code handles extracting them from the URL automatically.

### Smithy Concepts Used

**`resource`** — declares an entity with identifiers and lifecycle operations:

```smithy
resource List {
    identifiers: { listId: ListId, userId: UserId }
    read: GetList
    list: ListLists
}
```

**`operation`** with inline input/output — the `:=` syntax defines anonymous input/output shapes inline:

```smithy
operation GetList {
    input := for List {
        @required @httpLabel $listId
        @required @httpLabel $userId
    }
    output := for List { $name }
    errors: [ListeriaError]
}
```

`for List { $name }` means "bind to the resource's `name` property". The `$` prefix references a resource property.

**`@readonly`** — semantic annotation meaning the operation has no side effects.

**`@error("client")`** — marks a structure as an error shape that maps to a 4xx HTTP response.

---

## Day-to-Day Workflow

### Adding a new operation

1. Add the operation to the relevant `.smithy` file in `model/src/main/smithy/`
2. Add an `@http` binding to it
3. Add it to the resource's `operations` or lifecycle slot (`read`, `create`, `list`, etc.)
4. Run `./gradlew :model:assemble`
5. The new types appear in `input::`, `output::`, `error::`
6. Add the handler to `Listeria::builder(...)` in `src/main.rs`
7. Implement the handler function
8. Run `cargo check` to verify

### Changing an existing operation

1. Edit the `.smithy` file
2. Run `./gradlew :model:assemble` — the generated crate is overwritten
3. Fix any Rust compilation errors caused by the model change
4. Run `cargo check`

### Do not edit `todo-server-sdk/` by hand

The entire `todo-server-sdk/` directory is regenerated on every codegen run and is excluded from version control. The `.smithy` files are the source of truth. Any manual edits to the generated crate will be overwritten on the next `./gradlew :model:assemble`.

---

## How the Composite Build Resolves Dependencies

This is the trickiest part of the setup. smithy-rs is not published to Maven Central. So how does Gradle find `software.amazon.smithy.rust:codegen-server:0.1.16`?

When Gradle sees `includeBuild("smithy-rs")` in `settings.gradle.kts`, it scans the smithy-rs project and discovers that it publishes artifacts with the group `software.amazon.smithy.rust`. When any subproject in our build requests that group+artifact, Gradle substitutes the local smithy-rs source project instead of trying to download it.

This means smithy-rs gets compiled from Kotlin source as part of your build. It is slow the first time (a few minutes) but Gradle caches the compiled output in `~/.gradle/caches/` — subsequent runs are fast.

---

## The Big Picture

Here is the full pipeline in one view:

```
.smithy files          smithy-build.json         Gradle composite build
(your API spec)   →   (codegen config)      →   (compiles smithy-rs Kotlin
                                                  codegen from source)
                                                         │
                                                         ▼
                                                  smithy build runs
                                                  rust-server-codegen plugin
                                                         │
                                                         ▼
                                               todo-server-sdk/   ←─── generated Rust crate
                                               (typed inputs,           (do not edit)
                                                outputs, errors,
                                                service builder,
                                                protocol serde)
                                                         │
                                                         ▼
                                                  src/main.rs
                                               (your business logic)
                                               implements handler fns,
                                               wires into Listeria::builder
                                                         │
                                                         ▼
                                                  cargo build
                                               (normal Rust from here)
```

The division of responsibility is clean: Smithy owns the contract, the generated crate owns the HTTP layer, and your code owns the logic. You never touch serialization, routing, or protocol details — those are all generated. When the contract changes, you rerun `./gradlew :model:assemble`, the generated crate updates, and Rust's type system tells you exactly what in your code needs to change.
