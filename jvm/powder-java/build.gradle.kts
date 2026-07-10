plugins {
    `java-library`
    `maven-publish`
    signing
}

java {
    toolchain { languageVersion.set(JavaLanguageVersion.of(17)) }
    withSourcesJar()
    withJavadocJar()
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

publishing {
    publications {
        create<MavenPublication>("maven") {
            from(components["java"])
            artifactId = "powder-java"
            pom {
                name.set("powder-java")
                description.set("Java (JNI) binding for the Powder engine — a zero-copy columnar database client with a Rust core.")
                url.set("https://github.com/OSS-Ncode/powderORM")
                licenses { license { name.set("MIT"); url.set("https://opensource.org/licenses/MIT") } }
                developers { developer { id.set("oss-ncode"); name.set("Powder team") } }
                scm {
                    url.set("https://github.com/OSS-Ncode/powderORM")
                    connection.set("scm:git:https://github.com/OSS-Ncode/powderORM.git")
                }
            }
        }
    }
    repositories {
        maven {
            // Sonatype OSSRH staging. New Central accounts may instead use the
            // Central Portal — see PACKAGING.md. Credentials come from
            // ORG_GRADLE_PROJECT_ossrhUsername / _ossrhPassword (env in CI).
            name = "ossrh"
            url = uri(providers.gradleProperty("ossrhUrl")
                .getOrElse("https://s01.oss.sonatype.org/service/local/staging/deploy/maven2/"))
            credentials {
                username = providers.gradleProperty("ossrhUsername").orNull
                password = providers.gradleProperty("ossrhPassword").orNull
            }
        }
    }
}

// Sign only when a key is configured (skips local dev builds).
signing {
    isRequired = providers.gradleProperty("signingKey").isPresent
    useInMemoryPgpKeys(
        providers.gradleProperty("signingKey").orNull,
        providers.gradleProperty("signingPassword").orNull,
    )
    sign(publishing.publications["maven"])
}
