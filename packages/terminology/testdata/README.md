# 盲测集(覆盖率量化)

跑法:`cargo run -p terminology --example coverage -- packages/terminology/testdata/<file>.json`

每套数据都由**没看过 `dictionary.json` / `src/`** 的 subagent 凭中国医院报告单与处方的真实写法生成
(项名混排中英/缩写/括号、单位用报告写法、药名带盐基+剂型+规格+商品名)。命中率因此是诚实数字。

**但盲集一旦被用来找 miss、据此扩字典,它就不再盲**。所以下表标了每套的状态:凡是我看过 miss 清单
并据此改过字典/代码的,只能当**回归测试**用(防退化),不能再当验收数。**验收永远用一套全新的盲集。**

| 文件 | 状态 | 用途 |
|---|---|---|
| `blind_common_panels.json` | 已污染(调优依据) | 回归 |
| `blind_specialty_hard.json` | 已污染(调优依据) | 回归 |
| `blind_v2_common.json` | 已污染 | 回归 |
| `blind_v2_specialty.json` | 已污染 | 回归 |
| `blind_v3_accept.json` | 已污染 | 回归 |
| `blind_v4_final.json` | 已污染 | 回归 |
| `blind_v5_final.json` | 已污染 | 回归 |
| `blind_v6_final.json` | **干净**(2026-07-14 验收数出自此集) | 验收 |

下次扩字典 → 生成新盲集(v7)复测,并把 v6 移到「回归」。
