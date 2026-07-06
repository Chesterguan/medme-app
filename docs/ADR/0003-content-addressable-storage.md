# ADR 0003 · 内容寻址存储(CAS)+ 不可变原始层

Status: Accepted · Date: 2026-07-06

## Context
原则 3「Raw Never Dies」:原始文件永不被自动修改或删除,未来 AI 永远可重处理。需要一种存储既保证不可变、又天然去重、还能长期自解释。

## Decision
原始文件按 **`sha256(bytes)` 内容寻址**存入 CAS,元数据入 SQLite([003](../003_Core_Data_Model.md))。
- 布局:`objects/<hash[0:2]>/<hash[2:4]>/<hash>`,带 `VERSION` 文件。
- 写入原子(临时文件 + rename);已存在则跳过。**永不覆盖、永不自动删除。**
- `content_hash UNIQUE` → 去重天然;同文件多次/改名导入只存一份。
- 派生层(OCR/FTS/抽取)可随时从原文件重建。

## Alternatives 拒绝理由
- **原样目录存文件**:改名/移动破坏引用;去重需额外索引;易被外部误改。
- **文件塞进 SQLite BLOB**:单库膨胀、备份/同步笨重、违背"20 年后可打开"的轻量诉求。

## Consequences
- (+) 不可变 + 去重 + 可校验(hash 即完整性)一次拿到。
- (+) 备份/同步只需追加新对象(future 云同步友好)。
- (−) CAS 里文件名是 hash,不可人读;由 DB 的 `original_name` 兜底。
