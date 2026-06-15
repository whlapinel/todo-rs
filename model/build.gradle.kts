plugins {
    java
    id("software.amazon.smithy.gradle.smithy-base") version "1.3.0"
}

repositories {
    mavenCentral()
}

dependencies {
    "smithyBuild"("software.amazon.smithy.rust:codegen-server:0.1.16")
    "smithyBuild"("software.amazon.smithy.rust:codegen-client:0.1.14")
    "smithyBuild"("software.amazon.smithy:smithy-aws-traits:1.67.0")
    "smithyBuild"("software.amazon.smithy:smithy-model:1.67.0")
    "smithyBuild"("software.amazon.smithy.typescript:smithy-typescript-codegen:0.48.0")
    "smithyBuild"("software.amazon.smithy.typescript:smithy-aws-typescript-codegen:0.48.0")
}

tasks {
    val srcDir = projectDir.resolve("../")
    val serverSdkCrateName: String by project
    val clientCrateName: String by project
    val typescriptClientName: String by project
    register<Sync>("copyServerCrate") {
        dependsOn("smithyBuild")
        from(layout.buildDirectory.dir("smithyprojections/model/todo-server-sdk/rust-server-codegen/src"))
        into("$srcDir/$serverSdkCrateName/src")
    }
    register<Sync>("copyClientCrate") {
        dependsOn("smithyBuild")
        from(layout.buildDirectory.dir("smithyprojections/model/todo-client/rust-client-codegen/src"))
        into("$srcDir/$clientCrateName/src")
    }
    register<Sync>("copyTypescriptClient") {
        dependsOn("smithyBuild")
        from(layout.buildDirectory.dir("smithyprojections/model/todo-typescript-client/typescript-client-codegen/src"))
        into("$srcDir/$typescriptClientName/src")
    }
    named("assemble") {
        dependsOn("smithyBuild")
        finalizedBy("copyServerCrate")
        finalizedBy("copyClientCrate")
        finalizedBy("copyTypescriptClient")
    }
}
