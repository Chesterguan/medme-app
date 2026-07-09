<!--
  感谢贡献!请填写下面几项。中英文皆可。
  Thanks for contributing! Please fill in the sections below (Chinese or English).
-->

## 这个 PR 做了什么 / What does this PR do

<!-- 一两句概述改动和动机 / A sentence or two: what changed and why -->

## 关联 issue / Related issue

<!-- 例如 Closes #123 -->

## 改动类型 / Type of change

- [ ] Bug 修复 / Bug fix
- [ ] 新功能 / New feature
- [ ] 重构 / 内部改动 / Refactor / internal
- [ ] 文档 / Docs
- [ ] 其他 / Other:

## 自测 / Checklist

- [ ] `cargo fmt --all` 已运行,无格式差异 / ran `cargo fmt --all`
- [ ] `cargo clippy --all-targets -- -D warnings` 通过(相关 crate)/ clippy clean
- [ ] `cargo test` 通过(相关 crate)/ tests pass
- [ ] 前端改动:`pnpm -C apps/desktop exec tsc --noEmit` 与 `pnpm -C apps/desktop build` 通过 / frontend typecheck + build pass (if touched)
- [ ] 生产代码中没有 `unwrap()`(用 typed error 或有注释不变量的 `expect`)/ no `unwrap()` in production code
- [ ] 没有提交真实敏感病历或个人数据 / no real sensitive medical data committed
- [ ] 未改动 `apps/mobile/**`,除非本 PR 就是关于手机端 / did not touch `apps/mobile/**` unless this PR is about mobile

## 说明 / Notes

<!-- 截图、权衡、对隐私/存储/加密的影响等 / Screenshots, trade-offs, impact on privacy/storage/crypto -->
