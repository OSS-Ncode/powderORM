plugins {
    `java-library`
    id("com.vanniktech.maven.publish") version "0.37.0"
}

java {
    toolchain { languageVersion.set(JavaLanguageVersion.of(17)) }
}

// The Java sources live under crates/powder-java/java. PowderTest.java is a
// standalone smoke-test harness (default package, has main) — keep it out of
// the library jar.
sourceSets {
    main {
        java {
            setSrcDirs(listOf("../../crates/powder-java/java"))
            exclude("PowderTest.java")
        }
        // CI drops the JNI native libs here (one per platform) before publishing.
        // Loading is still via Powder.loadLibrary(path) / POWDER_LIB; bundling +
        // auto-extraction from resources is a follow-up (see PACKAGING.md).
        resources { setSrcDirs(listOf("src/main/resources")) }
    }
}

// Publishes to the Maven Central Portal (central.sonatype.com). The plugin adds
// the sources/javadoc jars, signs, and uploads. Credentials + signing key come
// from env in CI (see release.yml). Signing only runs when a key is present.
mavenPublishing {
    // vanniktech 0.31+: Central Portal is the only target, so this takes no argument.
    publishToMavenCentral()
    if (providers.gradleProperty("signingInMemoryKey").isPresent) {
        signAllPublications()
    }
    coordinates(project.group.toString(), "powder-orm-java", project.version.toString())
    pom {
        name.set("powder-orm-java")
        description.set("Java (JNI) binding for the Powder engine — a zero-copy columnar database client with a Rust core.")
        url.set("https://github.com/OSS-Ncode/powder-orm")
        licenses { license { name.set("MIT"); url.set("https://opensource.org/licenses/MIT") } }
        developers { developer { id.set("oss-ncode"); name.set("Powder team") } }
        scm {
            url.set("https://github.com/OSS-Ncode/powder-orm")
            connection.set("scm:git:https://github.com/OSS-Ncode/powder-orm.git")
            developerConnection.set("scm:git:ssh://git@github.com/OSS-Ncode/powder-orm.git")
        }
    }
}
