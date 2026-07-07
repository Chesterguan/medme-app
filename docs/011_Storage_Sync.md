# 011 · Storage & Sync · 存储与同步

> 真相源 = **不可变 CAS + append-only 事件日志**;SQLite DB 降为**派生缓存**(重放日志重建)。这样备份/同步/分享/审计全部变简单:同步两样 append-only 的东西,天然免冲突,不需要服务器。

关联:[003_Core_Data_Model](003_Core_Data_Model.md) · [004_Import_Pipeline](004_Import_Pipeline.md) · [009_Encounter_Model](009_Encounter_Model.md) · 记忆 `medme-real-data-sources`

---

## 1. 分层(真相源 vs 派生)

```
真相源(不可变 / append-only,永存、可同步):
  objects/           内容寻址 CAS(原始文件 + 大的派生产物如 OCR 文本,按 sha256)
  log/               append-only 事件日志(小事件,引用 CAS 哈希)

派生缓存(可随时删掉重建):
  medme.db           SQLite(document / ocr_result / encounter / FTS)= 重放 log 得到
```

- **只有 objects/ 和 log/ 需要备份/同步**;`medme.db` 各端本地重放重建。
- 呼应既有原则:Raw Never Dies(CAS)+ 派生可重建(我们的 encounter/FTS 已如此,这里推到 DB 整体)。

## 2. 事件日志格式

- `log/` 下按段存 append-only JSONL(如 `log/000001.jsonl`,写满滚动);每行一个事件。
- 事件 = 不可变、带**内容寻址 id**(`event_id = sha256(canonical_json)`),便于合并去重。
- 排序:每事件带 `ts`(RFC3339)+ `device_id` + `seq`;合并两端日志 = 事件并集(按 id 去重)→ 按 (ts, device_id, seq) 稳定排序 → 重放。确定性、幂等。
- 事件类型(v1 最小集):
  - `file_imported { content_hash, original_name, mime, byte_size, ts, device_id }` —— 对应 source_file;原文件已在 CAS。
  - `document_recognized { content_hash, doc_type, doc_date?, doc_date_end?, title?, language?, page_count, ocr_backend, ocr_text_hash }` —— parser/OCR 结果;**OCR 文本存 CAS**(可能大),事件只引用其哈希。
  - `document_deleted { content_hash }` / `annotation_added {...}`(未来)。
- **encounter 不进日志**(纯派生,重放后由 `rebuild_encounters` 计算)。

## 3. 重放 / 物化(materialize)

- `materialize(objects, log) -> medme.db`:清空派生表 → 按序重放事件 → 建 document/ocr_result/FTS → `rebuild_encounters`。幂等。
- 触发时机:首次打开、日志变化(本地写入或同步拉取后)。可增量(记录已处理到的 event_id 水位)。
- `medme.db` 损坏/丢失 → 直接重建,零数据损失(真相在 objects+log)。

## 4. 导入路径改造

现在:导入 → 直接写 DB。改为:
```
文件 → 存 CAS → 追加 file_imported 事件 → parser/OCR → 存 ocr_text 到 CAS → 追加 document_recognized 事件 → materialize(增量)
```
CLI / Tauri / Watch Folder 都只"追加事件 + 物化",不再直接写业务表。

## 5. 加密

- v0.1:依赖系统全盘加密(macOS FileVault)。
- 以后:可选**口令加密**——用口令派生密钥,加密 CAS 对象 + 日志段(SQLCipher 给 DB 或直接不加密 DB,因为 DB 可重建)。密钥不落盘明文。属加分项,不阻塞。

## 6. 位置 / 备份

- 默认 `app-data/vault/`;**允许用户自定义 vault 路径**(设置页),可指向 Documents 或**云同步文件夹**(iCloud Drive / OneDrive / WebDAV 挂载点)。
- 备份 = 复制 vault 文件夹(或只 objects/+log/)。CAS 哈希自校验完整性。
- ⚠️ **DB 文件不要直接云同步**(并发/半同步损坏);同步 objects/+log/,DB 本地重建。

## 7. 手机上传(先做,无需原生 App)

- 手机用**原生扫描 / 微信扫一扫 / 相册** → 保存到**共享云文件夹的 inbox/**(如 iCloud Drive/MedMe/Inbox)。
- 桌面 **Watch Folder** 监听 inbox → 自动摄入(存 CAS + 事件 + 物化)+ OCR。复用 [004](004_Import_Pipeline.md) 的 Watch Folder + 现有 OCR。
- 以后:Tauri v2 mobile 原生采集 App,直接追加事件到共享 vault。

## 8. 同步(无服务器)

- 桌面/手机指向**同一个共享文件夹**(云盘)。各端只读写 objects/+log/(append-only)→ 云盘做文件同步 → 各端 materialize 重放。
- 合并:日志事件按内容 id 去重、稳定排序重放 → **无冲突**(append-only + CAS 不可变)。
- 不需要自建服务器 / 账号(patient-first / local-first 不破)。

## 9. 分享(链接 + 公开无数据网页查看器,端到端加密)

- **生成分享**:选范围(按接受者角色:给医生=相关记录/某次就诊;全量;等)→ 打包成自包含 bundle(选中文档的 CAS 对象 + 一个精简 manifest/JSON)。
- **端到端加密**:bundle 用随机密钥加密;**密文**上传到公开托管(或做成一份文件);**密钥放分享链接的 `#fragment`**(不发服务器)。
- **查看器 = 公开静态网页,零数据**:按链接拉密文 → 用 fragment 里的密钥**客户端解密** → 渲染时间线/文档。托管方只见密文。
- **加固**:短信验证码二次门禁;链接**有效期 + 可撤销**;只读。
- 与**角色化导出**同源:导出 = 生成 bundle(PDF/FHIR/JSON);分享 = 同一 bundle 走加密链接。

## 10. 阶段

- **本次(现在)**:重构存储为 **CAS + append-only 日志 + 派生 DB + materialize**;导入路径改为追加事件;DB 可重建。(不含加密/同步/分享,只打地基。)
- 接续:Watch Folder inbox(手机上传);可选口令加密;云文件夹同步;角色化导出(v1 全量 PDF);分享链接 + 静态查看器 + E2E + 短信。
- 手机原生 App(Tauri mobile)更后。

## 11. 兼容

- 现有 vault(纯 DB,无日志):迁移 = 从当前 DB 反向生成初始事件日志(把已有 source_file/document/ocr 导出成 file_imported/document_recognized 事件),之后正常运行。一次性。
