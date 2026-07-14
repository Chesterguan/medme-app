//! 覆盖率验证:拿一份「不看字典生成」的真实中文报告(subagent 产出),跑
//! `terminology::normalize`,统计分析物名/药名/单位的命中率 + 未命中清单。
//! 用法:`cargo run -p terminology --example coverage -- <demo_reports.json>`
//!
//! 命中率是**诚实数字**:数据由不知道 dictionary.json 内容的 subagent 生成。

use std::collections::HashMap;

use serde::Deserialize;
use terminology::{normalize_unit, resolve, resolve_drug, Dictionary};

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
        let mut us: Vec<String> = e.units.iter().map(|u| normalize_unit(&u.unit)).collect();
        if let Some(cu) = &e.canonical_unit {
            us.push(normalize_unit(cu));
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
            // 提取层入口:拆候选 + 用单位证据/最长匹配择优(不再「第一个命中即用」)。
            let hit = resolve(&it.name, Some(&it.unit));
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
                            .is_some_and(|us| us.iter().any(|u| *u == normalize_unit(&it.unit)));
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
            // 先候选拆分(去括号商品名/尾部规格),再 normalize(内部剥盐基/剂型)。
            match resolve_drug(&d.name) {
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
        "化验分析物名:  {}/{} 命中  = {:.1}%   (其中非精确命中<1.0:剥壳0.8/OCR混淆0.5: {})",
        lab_hit,
        lab_n,
        pct(lab_hit, lab_n),
        lab_ocr
    );
    println!(
        "药品名:        {}/{} 命中  = {:.1}%   (其中非精确命中<1.0:剥壳0.8/OCR混淆0.5: {})",
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
