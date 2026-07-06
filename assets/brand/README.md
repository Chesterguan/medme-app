# MedMe 品牌资产 · Brand Assets

## 文件
- `medme-logo-sheet.jpg` — **主品牌图纸**(用户提供)。包含:标志图形(心形 + 盾牌医疗十字 + 保险箱刻度盘,蓝 + 青绿)+ 两版字标锁定("MedMe 医我:个人数据保险箱" / "MedMe Personal Data Vault")+ 标注文字。

## 正确用法(重要)
这是**设计图纸,不是可直接投放的资产**:白底、含标注文字("CHINESE/ENGLISH VERSION")。**不要**整张 JPG 当图标/favicon/侧栏 logo 用。

到 **Plan C(Tauri + React 外壳)** 时再从图纸切出干净资产:
- 独立标志图形 → 透明底 SVG(优先)/ PNG。
- 字标锁定(中/英两版)→ 独立 SVG/PNG。
- Tauri 应用图标多尺寸(`.icns` / `.ico` / png 集)由独立标志生成。
- 用途落点:应用图标、侧栏品牌区(替换原型里占位的 lucide `ShieldCheck`)、加载页。

ponytail:现在**不**做切图/矢量化/多尺寸生成——UI 尚不存在,属投机工作。留到 Plan C。

## 配色(取自标志,供 Plan C 校准)
- 主蓝:标志用的是偏亮的天蓝/青蓝(比原型的 tailwind `blue-600` 更亮)。Plan C 定主色时二选一:向标志的青蓝靠(`sky-500`/`blue-500`)或保持 `blue-600`,择一并统一。
- 辅绿:青绿/翡翠(`emerald`/`teal`),与信任/本地化/正常值语义一致。
- 详见 UI 设计语言记忆(项目 memory `medme-ui-design-language`)。
