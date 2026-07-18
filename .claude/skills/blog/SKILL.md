---
name: blog
description: 写/改 MedMe 公开开发日志(gh-pages 的 blog.html,专题卡片制)。当用户说「写篇 blog」「加个专题」「更新开发日志」「把这块写进 blog」时使用。
---

# 开发日志

公开出口是 gh-pages 分支的 `blog.html`,专题卡片制:一张卡片 = 一个真实难题,卡内按 PR 合并日期(UTC)倒序记迭代过程。

**视角:有工程背景的 marketing。记录过程、经验、决定,不评判产品好坏。**

## 现有专题

OCR / 抽取与术语 / 数据存在哪 / 给医生的那一页 / 系统设计(手机优先)

新内容优先**并进已有专题的时间线**,不要动辄开新卡 —— 只有真正独立的课题才开新专题。

## 怎么做

1. 准备 worktree(gh-pages 不是日常工作分支):
   ```
   git worktree add <tmp>/ghp gh-pages
   ```
2. 派 `blog-writer` subagent 写。它自带全部语气与禁写规则(见 `.claude/agents/blog-writer.md`),把任务和素材来源给它:
   - 要写的专题/条目
   - 相关 PR 号(`gh pr list --state merged --json number,title,mergedAt`)
   - `docs/log/` 最新几条、相关 `docs/ADR/`
3. **审核必须由另一个 subagent 做,不许自审。** 要求逐条核到 `file:line`,判定证实/证伪/无法证实/表述过强,给改法。落实后再交付。
4. 自检:标签配平、语言切换中英互斥、无横向溢出(可起 `python3 -m http.server` + playwright 实测)。
5. **不推送。** 推送(`git push origin gh-pages`)由用户决定。

## 反复踩过的坑(别再踩)

- 把内部工程视角当用户视角:「还没做完/还没测够」被写成产品缺陷。
- 删了测试来源却留着测试结论 —— 变成没出处的负面断言,比原来更糟。
- 平台差异当成缺一块:先想清楚是「没做」还是「不需要做」(例:手机不阅片,自然不带 DICOM 像素解码)。
- 前后卡片自相矛盾:写「核心都在一份 Rust 代码里共用」时,忘了 OCR 在移动端走 Dart 原生。
