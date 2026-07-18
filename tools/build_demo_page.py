#!/usr/bin/env python3
"""从规范查看器 + demo-payload.json 生成示例查看器页面。
与 share.rs 注入密文同一机制:把 payload 塞进非执行的 JSON 数据节点。
用法: python3 build_demo_page.py <viewer.html> <payload.json> <out.html>
"""
import json, sys
viewer, payload_path, out = sys.argv[1], sys.argv[2], sys.argv[3]
html = open(viewer, encoding="utf-8").read()
payload = json.load(open(payload_path, encoding="utf-8"))
MARK = "<!--DEMO_DATA_SLOT-->"
if MARK not in html:
    sys.exit("查看器缺少 DEMO_DATA_SLOT 注入点")
# </script> 在 JSON 里会提前闭合脚本标签,按 HTML 规范转义
blob = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/")
node = '<script type="application/json" id="demo-data">' + blob + "</script>"
open(out, "w", encoding="utf-8").write(html.replace(MARK, node, 1))
print(f"{out}: {len(open(out, encoding='utf-8').read())/1024:.0f} KB")
