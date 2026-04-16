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
