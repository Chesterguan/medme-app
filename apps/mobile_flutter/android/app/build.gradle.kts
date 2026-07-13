plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "com.medme.mobile"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "com.medme.mobile"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    buildTypes {
        release {
            // 内测阶段先用 debug 签名(CI 出可侧载 APK);正式上架前换正式 keystore。
            signingConfig = signingConfigs.getByName("debug")
            // R8 需要额外规则:ML Kit 文字识别插件把中/日/韩/梵文识别器都声明成
            // compileOnly(只打包 Latin),我们只加了中文包,其余脚本类缺失需 -dontwarn。
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

dependencies {
    // ML Kit 中文文字识别包。插件默认只打包 Latin;MedMe 面向中文病历,必须显式加
    // 中文识别器,否则中文 OCR 运行时不工作、且 release R8 会因缺类构建失败。
    // 日/韩/梵文用不到,不加(避免无谓增大 APK),对应缺类在 proguard-rules.pro 里 -dontwarn。
    implementation("com.google.mlkit:text-recognition-chinese:16.0.1")
}
