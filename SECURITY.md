# 安全策略 / Security Policy

MedMe(医我)处理的是**个人医疗数据**,安全与隐私是产品的核心。
如果你发现了安全漏洞,请负责任地私下上报,给我们时间修复后再公开。

MedMe handles **personal medical data**, so security and privacy are core to the
product. If you find a vulnerability, please disclose it responsibly and
privately so we can fix it before it becomes public.

## 私下上报 / Reporting a vulnerability

**请勿在公开 issue、PR 或讨论区提交安全漏洞。**
**Do NOT open a public issue, PR, or discussion for a security vulnerability.**

请发送邮件至 / Email:

> **chesterfield199512@gmail.com**

请尽量包含 / Please include, where possible:

- 漏洞描述与影响 / a description of the issue and its impact
- 复现步骤或 PoC(**请勿附带真实敏感病历**,用示例/脱敏数据)/ steps to
  reproduce or a proof of concept (do **not** attach real sensitive records —
  use demo or redacted data)
- 受影响的版本 / 平台 / affected version and platform
- 你认为的严重程度 / your assessment of severity

你也可以通过 GitHub 的 **Private vulnerability reporting**(仓库 Security 标签页)
上报。/ You may also use GitHub's **Private vulnerability reporting** from the
repository's Security tab.

## 响应预期 / What to expect

这是一个小团队维护的开源项目,我们会尽力做到 / This is a small,
volunteer-maintained open-source project; we aim to:

- **3 个工作日内**确认收到 / acknowledge your report within **3 business days**
- 与你一起评估与复现 / work with you to assess and reproduce the issue
- 在修复发布后进行公开披露,并(在你同意的前提下)致谢 / disclose publicly once a
  fix ships, and credit you if you wish

我们没有付费漏洞赏金计划。/ We do not run a paid bug-bounty program.

## 支持的版本 / Supported versions

我们只对**最新发布版本**提供安全修复。请始终更新到最新版。

Security fixes are provided for the **latest released version** only. Please stay
up to date.

| 版本 / Version | 支持 / Supported |
| --- | --- |
| latest (`v1.0` 及之后 / and later) | ✅ |
| 更早版本 / older | ❌ |

## 范围 / Scope

MedMe 是**本地优先、零服务器**的应用,威胁模型也因此不同于云服务。我们特别关注:

MedMe is a **local-first, zero-server** application, so the threat model differs
from a cloud service. We are particularly interested in:

- **端到端加密分享 (E2E share)** 的加密实现(`packages/share`,AES-256-GCM +
  口令派生 / password-derived key):密文可被解密、口令/密钥泄露、降级、
  可预测 nonce、离线暴力破解等 / weaknesses in the share crypto: plaintext
  recovery, key/password leakage, downgrade, predictable nonce, offline brute
  force
- **在线预览器 (hosted viewer)**(`web/hosted-viewer` /
  https://chesterguan.github.io/medme-viewer/):在浏览器中本地解密时的 XSS /
  数据外泄 / client-side decryption XSS or exfiltration in the browser viewer
- **导入/解析管线**(OCR、PDF、DICOM 解析):处理不可信文件时的内存安全 /
  路径穿越 / 拒绝服务 / memory-safety, path traversal, or DoS when parsing
  untrusted files
- **审计与事件日志**(内容寻址存储 + 追加式 JSONL):完整性 / 可被篡改而不被
  发现 / integrity of the content-addressed store and append-only log

**不在范围内 / Out of scope**:MedMe 没有后端服务器,因此没有服务器端漏洞。
默认数据存在用户本机,**数据在设备/云盘上的保密性由用户自己掌控**(见下)。

## 你持有你自己的数据 / You hold your own data

MedMe 默认把你的数据保存在**你自己的设备**上,同步靠**你自己的云盘**。
我们看不到、也不持有你的数据。这意味着 / MedMe stores your data on **your own
device** and syncs via **your own cloud drive**. We cannot see or hold your
data. That means:

- 设备的物理/账户安全由你负责(建议开启 FileVault + iCloud 高级数据保护,
  应用内「数据安全引导」有说明)/ device and account security is on you
  (we recommend enabling full-disk encryption; the in-app security guide helps)
- 加密分享的**口令**由你保管并单独告知对方;口令一旦泄露,持有分享文件的人即可
  解密 / you hold the share **password** and pass it out of band; anyone with
  both the file and the password can decrypt it
- 我们无法帮你找回丢失的数据或口令(去中心化、零服务器的代价)/ we cannot
  recover lost data or passwords

> ⚠️ 提醒:MedMe 是数据整理工具,非医疗器械,不提供医疗诊断或建议。
