//! 覆盖率验证:拿一份「不看字典生成」的真实中文报告(subagent 产出),跑
//! `terminology::normalize`,统计分析物名/药名/单位的命中率 + 未命中清单。
//! 用法:`cargo run -p terminology --example coverage -- <demo_reports.json>`
//!
//! 命中率是**诚实数字**:数据由不知道 dictionary.json 内容的 subagent 生成。

use std::collections::HashMap;

use serde::Deserialize;
use terminology::{normalize, Dictionary};

#[derive(Deserialize)]
struct Demo {
    reports: Vec<Report>,
}
#[derive(Deserialize)]
struct Report {
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    drugs: Vec<Drug>,
}
#[derive(Deserialize)]
struct Item {
    name: String,
    #[serde(default)]
    unit: String,
}
#[derive(Deserialize)]
struct Drug {
    name: String,
}

/// 复合化验名 → 候选:去括号内容 + 括号内(可能是同义缩写)+ 按空格/斜杠切 token。
/// 真实报告写「甘油三酯 TG」「皮质醇(8AM)」「肌酐 Cr(Scr)」,精确查表整串必 miss,
/// 拆成 token 后各自去查。纯确定性,不是模型。
fn lab_candidates(name: &str) -> Vec<String> {
    let is_open = |c: char| matches!(c, '(' | '（' | '[' | '【');
    let is_close = |c: char| matches!(c, ')' | '）' | ']' | '】');
    // 去括号后的主体 + 收集括号内内容
    let (mut stripped, mut inner_all) = (String::new(), Vec::<String>::new());
    let mut inner = String::new();
    let mut depth = 0i32;
    for c in name.chars() {
        if is_open(c) {
            depth += 1;
        } else if is_close(c) {
            depth -= 1;
            if depth <= 0 && !inner.trim().is_empty() {
                inner_all.push(inner.trim().to_string());
                inner.clear();
            }
        } else if depth >= 1 {
            inner.push(c);
        } else {
            stripped.push(c);
        }
    }
    let mut cands: Vec<String> = vec![stripped.trim().to_string()];
    let seps = [' ', '\u{3000}', '/', '／', '、', ',', '，'];
    for t in stripped.split(seps) {
        let t = t.trim();
        if !t.is_empty() {
            cands.push(t.to_string());
        }
    }
    for blk in inner_all {
        for t in blk.split(seps) {
            let t = t.trim();
            if !t.is_empty() {
                cands.push(t.to_string());
            }
        }
    }
    cands.retain(|c| !c.is_empty());
    cands.dedup();
    cands
}

/// 单位比较用的宽松归一:去空格、小写、µ/μ→u、×→x、去 ^,好让 UCUM(10*9/L)
/// 和报告写法(×10^9/L)尽量对上;仍对不上就算 miss(多是记法差异,可补数据)。
fn nu(u: &str) -> String {
    u.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'µ' | 'μ' => 'u',
            '×' => 'x',
            _ => c,
        })
        .filter(|&c| c != '^')
        .collect()
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: cargo run -p terminology --example coverage -- <demo_reports.json>");
    let demo: Demo = serde_json::from_str(&std::fs::read_to_string(&path).expect("读 demo 文件"))
        .expect("解析 demo JSON");
    let dict: Dictionary =
        serde_json::from_str(include_str!("../dictionary.json")).expect("解析 dictionary.json");

    // key -> 该条目接受的所有单位(canonical + 各转换单位),供单位命中判断。
    let mut units: HashMap<String, Vec<String>> = HashMap::new();
    for e in &dict.entries {
        let mut us: Vec<String> = e.units.iter().map(|u| nu(&u.unit)).collect();
        if let Some(cu) = &e.canonical_unit {
            us.push(nu(cu));
        }
        units.insert(e.key.clone(), us);
    }

    let (mut lab_n, mut lab_hit, mut lab_ocr) = (0u32, 0u32, 0u32);
    let (mut unit_n, mut unit_hit) = (0u32, 0u32);
    let (mut drug_n, mut drug_hit, mut drug_ocr) = (0u32, 0u32, 0u32);
    let mut lab_miss: Vec<String> = vec![];
    let mut unit_miss: Vec<String> = vec![];
    let mut drug_miss: Vec<String> = vec![];

    for r in &demo.reports {
        for it in &r.items {
            lab_n += 1;
            // 复合名先拆候选,任一 token 命中即算(确定性)。
            let hit = lab_candidates(&it.name).iter().find_map(|c| normalize(c));
            match hit {
                Some(m) => {
                    lab_hit += 1;
                    if m.confidence < 1.0 {
                        lab_ocr += 1;
                    }
                    if !it.unit.trim().is_empty() {
                        unit_n += 1;
                        let ok = units
                            .get(&m.key)
                            .is_some_and(|us| us.iter().any(|u| *u == nu(&it.unit)));
                        if ok {
                            unit_hit += 1;
                        } else {
                            unit_miss.push(format!("{} [{}]", it.name, it.unit));
                        }
                    }
                }
                None => lab_miss.push(it.name.clone()),
            }
        }
        for d in &r.drugs {
            drug_n += 1;
            // normalize 内部已做确定性剥壳(盐基/剂型)。
            match normalize(&d.name) {
                Some(m) => {
                    drug_hit += 1;
                    if m.confidence < 1.0 {
                        drug_ocr += 1;
                    }
                }
                None => drug_miss.push(d.name.clone()),
            }
        }
    }

    let pct = |a: u32, b: u32| {
        if b == 0 {
            0.0
        } else {
            a as f64 * 100.0 / b as f64
        }
    };
    println!(
        "字典版本: {} · 条目数: {}",
        dict.version,
        dict.entries.len()
    );
    println!("──────────────────────────────────────────────");
    println!(
        "化验分析物名:  {}/{} 命中  = {:.1}%   (其中疑似 OCR/0.5 置信: {})",
        lab_hit,
        lab_n,
        pct(lab_hit, lab_n),
        lab_ocr
    );
    println!(
        "药品名:        {}/{} 命中  = {:.1}%   (其中疑似 OCR/0.5 置信: {})",
        drug_hit,
        drug_n,
        pct(drug_hit, drug_n),
        drug_ocr
    );
    println!(
        "单位(命中项):  {}/{} 匹配  = {:.1}%   (记法差异也算 miss,见下)",
        unit_hit,
        unit_n,
        pct(unit_hit, unit_n)
    );
    println!("──────────────────────────────────────────────");
    let show = |title: &str, v: &[String]| {
        println!("\n【{} · {} 条】", title, v.len());
        for x in v {
            println!("  ✗ {x}");
        }
    };
    show("未命中·化验分析物", &lab_miss);
    show("未命中·药品", &drug_miss);
    show("单位对不上", &unit_miss);
}
