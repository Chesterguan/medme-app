# `src/main/jniLibs/arm64-v8a/libc++_shared.so` 是什么、为什么在这

那份 `.so` 不是我们编译产物,是从 Android NDK 拷来的 C++ 标准库共享库(NDK
官方推荐做法——进程里所有原生库共享同一份 `libc++`,而不是各自静态内嵌一份;
这台 app 里 ML Kit 自己的 `libmlkit_google_ocr_pipeline.so` 也是靠这个模式
跑的)。这份说明文档特意放在 jniLibs 目录**外面**(`rust_builder/android/`
模块根)——jniLibs 源目录下的内容会被 AGP 按 ABI 子目录名整个打进 APK 的
native libs,不想让一个说明文档掺进那条打包路径,哪怕大概率无害。

## 为什么要手动放这一份

`feat/android-pp-ocr` 分支把安卓图片 OCR 接到 PP-OCRv5(`packages/ocr` 的
`engine` 路径,oar-ocr + ONNX Runtime,和 iOS 同一引擎)。`ort` 在安卓上动态
链接 `libc++_shared.so`(`llvm-readelf -d` 能看到我们编译出的
`librust_lib_mobile_flutter.so` 里有一条 `NEEDED libc++_shared.so`)。cargokit
的 Gradle 插件(`rust_builder/cargokit/gradle/plugin.gradle`)只把我们自己编译
出的 `lib*.so` 拷进 APK 的 jniLibs(见 cargokit 的
`build_tool/lib/src/artifacts_provider.dart` `getArtifactNames`),不知道、也不
会带上这个 NDK 系统库——不手动补上,首次真机跑会在系统 `dlopen` 我们的 `.so`
时因为找不到 `libc++_shared.so` 而 `UnsatisfiedLinkError` 崩溃。

## 这份文件的来历(可复现)

从构建实际用的 NDK 版本(`28.2.13676358`,Flutter 3.44.6 的默认
`ndkVersion`,CI 用的是同一个版本)拷出、剥离调试符号:

```sh
NDK=~/Library/Android/sdk/ndk/28.2.13676358
cp "$NDK/toolchains/llvm/prebuilt/<host>/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so" .
"$NDK/toolchains/llvm/prebuilt/<host>/bin/llvm-strip" --strip-unneeded libc++_shared.so
```

原始 9.24MB(带调试信息)→ 剥离后 1.25MB。`llvm-readelf --dyn-syms` 确认剥离后
仍有 2502 条动态符号、`llvm-readelf -d` 确认 `SONAME` 仍是 `libc++_shared.so`
——运行时的动态链接器按 SONAME 匹配,不依赖文件名之外的任何东西,剥离不影响
链接。

## 为什么放在这个目录

这个模块(`rust_builder/android`,`rootProject.name = rust_lib_mobile_flutter`)
经 `pubspec.yaml` 的 `rust_lib_mobile_flutter: {path: rust_builder}` 注册成
Flutter 插件,Flutter 的插件加载机制会把它的 android 子模块自动接进主 app 的
Gradle 构建——和 `google_mlkit_text_recognition` 接入自己的原生依赖是同一条
机制。cargokit 的 `plugin.gradle` 已经在往这个**同一个模块**的
`debug`/`release` sourceSet 动态写 jniLibs(编译产物,不进 git);这里
`src/main/jniLibs/` 是 AGP 默认的**静态** jniLibs 源目录约定,同一模块的
`main` sourceSet 会和 `debug`/`release` sourceSet 一起按 variant 汇总进最终
APK——两者都往同一份 jniLibs 集合里加东西,机制上没有冲突。

**没有跑整包 `flutter build apk` 验证这份文件真的进了最终 APK 的
`lib/arm64-v8a/`**——按 `apps/mobile_flutter/CLAUDE.md` 的构建纪律,完整
Android 构建是 CI 的活,不在本地日常验证范围内。这一步等 CI/真机验收。

## 只有 arm64-v8a

只验证过 `aarch64-linux-android` 一个交叉编译 target(主流真机 ABI)。要支持
`armeabi-v7a`/`x86_64`(比如模拟器),照上面的方法从 NDK 对应的
`toolchains/llvm/prebuilt/<host>/sysroot/usr/lib/<triple>/libc++_shared.so`
拷一份放到对应的 `<abi>/` 目录,同时确认 `apps/mobile_flutter/rust` 那边也
为该 target 交叉编译过、链接过。
