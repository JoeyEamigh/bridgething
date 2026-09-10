plugins {
  id("com.android.library")
  kotlin("android")
  kotlin("plugin.serialization")
}

android {
  namespace = "com.bridgething.companion"
  compileSdk = 36

  defaultConfig {
    minSdk = 26
    testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
  }

  compileOptions {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
  }

  testOptions {
    unitTests {
      isIncludeAndroidResources = false
      isReturnDefaultValues = true
    }
  }
}

kotlin {
  jvmToolchain(21)
  compilerOptions {
    jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
  }
}

dependencies {
  api(project(":packages:companion:kotlin:core"))
  api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
  api("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
  api("androidx.core:core-ktx:1.13.1")
  implementation("androidx.media:media:1.8.0")
  implementation("androidx.media3:media3-exoplayer:1.11.0")
  implementation(project(":packages:asr:kotlin:whisper"))
  implementation("io.ktor:ktor-client-core:3.0.0")
  implementation("io.ktor:ktor-client-cio:3.0.0")
  implementation("io.ktor:ktor-client-websockets:3.0.0")
  implementation("com.google.ai.edge.litert:litert:2.1.6")
  implementation("androidx.security:security-crypto:1.1.0")
  compileOnly("com.google.android.gms:play-services-location:21.3.0")
  testImplementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.8.0")
  testImplementation("net.java.dev.jna:jna:5.17.0")
  testImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
  testImplementation("io.mockk:mockk:1.13.13")
  testImplementation("io.ktor:ktor-server-cio:3.0.0")
  testImplementation("io.ktor:ktor-server-websockets:3.0.0")
  testRuntimeOnly("org.junit.platform:junit-platform-launcher")
  androidTestImplementation("androidx.test.ext:junit:1.2.1")
  androidTestImplementation("androidx.test:runner:1.6.2")
  androidTestImplementation("androidx.test:core-ktx:1.6.1")
  androidTestImplementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.8.0")
  androidTestImplementation("com.google.android.gms:play-services-location:21.3.0")
}

tasks.withType<Test>().configureEach {
  useJUnitPlatform()
}

apply(from = "$projectDir/../../../../gradle/companion-core-ffi-tests.gradle.kts")
