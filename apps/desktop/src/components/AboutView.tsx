import { useState } from "react";
import { ShieldCheck } from "lucide-react";

interface Section {
  t: string;
  b: string;
}

const ZH: Section[] = [
  {
    t: "一、软件宗旨与性质",
    b: "MedMe(医我)是一款个人医疗数据管理工具,旨在帮助用户收集散落于各医疗机构的个人医疗数据,并聚合为可由用户自行掌控、可随身携带的个人电子数据保险箱。本软件为数据整理与保存工具,非医疗器械,亦非诊疗系统。",
  },
  {
    t: "二、非医疗建议",
    b: "本软件不提供任何形式的医疗诊断、治疗建议、用药指导或健康决策。软件内由光学字符识别(OCR)、自动归类、结构化展示等功能生成的内容,仅为对原始资料的辅助整理,可能存在识别或归类错误。任何医疗决策请以原始医疗文件为准,并咨询具备执业资质的医疗专业人员。开发者不对基于本软件内容作出的任何医疗决策承担责任。",
  },
  {
    t: "三、数据访问与授权",
    b: "您的医疗数据属于您本人。仅在您本人、或依法对您负有监护/代理职责的法定责任人明确同意的前提下,方可调取、查看或分享数据。您对通过本软件生成的任何分享链接、导出文件及其传播后果自行负责。",
  },
  {
    t: "四、数据存储与所有权",
    b: "本软件采用本地优先(local-first)、去中心化的存储方式。所有数据存储于由账号所有者自行设置的可信本地设备或云端位置,由您完全掌控。开发者不集中持有、不上传、不访问您的医疗数据。",
  },
  {
    t: "五、数据安全与责任限制",
    b: "您应自行备份数据并妥善保管访问凭据与设备。若怀疑数据发生遗失、损坏或泄露,可联系开发者寻求技术协助;但因本软件为去中心化存储、开发者不持有您数据的副本,我们可能无法为您找回或恢复任何数据。在适用法律允许的最大范围内,开发者不对数据的遗失、损坏、泄露或任何间接损失承担赔偿责任。",
  },
  {
    t: "六、其他",
    b: '本软件按"现状"提供,不作任何明示或默示担保。开发者保留随时更新本声明的权利。您安装、访问或使用本软件,即表示已阅读、理解并同意本声明的全部内容。',
  },
];

const EN: Section[] = [
  {
    t: "1. Purpose & Nature",
    b: "MedMe is a personal medical-data management tool that helps you collect medical data scattered across healthcare institutions and aggregate it into a portable personal data vault under your own control. It is a tool for organizing and preserving data — not a medical device, and not a diagnostic or treatment system.",
  },
  {
    t: "2. No Medical Advice",
    b: "The software provides no medical diagnosis, treatment advice, medication guidance, or health decisions of any kind. Content produced by OCR, automatic classification, and structured display is an aid to organizing your original materials and may contain recognition or classification errors. Base any medical decision on the original medical documents and consult a licensed healthcare professional. The developer bears no responsibility for decisions made based on the software's content.",
  },
  {
    t: "3. Data Access & Consent",
    b: "Your medical data belongs to you. It may be accessed, viewed, or shared only with the explicit consent of you or a legally responsible party (guardian / representative). You are solely responsible for any share links or exported files you create and their consequences.",
  },
  {
    t: "4. Storage & Ownership",
    b: "The software is local-first and decentralized. All data is stored on trusted local devices or cloud locations configured by the account owner and remains under your control. The developer does not centrally hold, upload, or access your medical data.",
  },
  {
    t: "5. Security & Limitation of Liability",
    b: "You are responsible for backing up your data and safeguarding your access credentials and devices. If you suspect data loss, corruption, or leakage, you may contact the developer for technical help; however, because storage is decentralized and the developer holds no copy of your data, we may be unable to recover it. To the maximum extent permitted by law, the developer is not liable for loss, corruption, leakage, or any indirect damages.",
  },
  {
    t: "6. General",
    b: 'The software is provided "as is" without warranties of any kind. The developer may update this statement at any time. By installing, accessing, or using the software, you acknowledge that you have read, understood, and agreed to all of its terms.',
  },
];

// 隐藏的「审计/管理员」入口:短时间内连点版本号 5 次即可进入(仿 Android
// 开发者模式的「点击 7 次」套路)。不出现在正式导航里,普通用户不会误触。
const HIDDEN_TAP_COUNT = 5;
const HIDDEN_TAP_WINDOW_MS = 3000;

export default function AboutView({ onNav }: { onNav: (id: string) => void }) {
  const [lang, setLang] = useState<"zh" | "en">("zh");
  const sections = lang === "zh" ? ZH : EN;
  const [tapCount, setTapCount] = useState(0);
  const [lastTap, setLastTap] = useState(0);

  const onVersionClick = () => {
    const now = Date.now();
    const withinWindow = now - lastTap < HIDDEN_TAP_WINDOW_MS;
    const next = withinWindow ? tapCount + 1 : 1;
    setLastTap(now);
    setTapCount(next);
    if (next >= HIDDEN_TAP_COUNT) {
      setTapCount(0);
      onNav("audit");
    }
  };

  return (
    <div className="flex-1 overflow-y-auto bg-slate-50 p-6 md:p-10">
      <div className="max-w-2xl mx-auto space-y-6">
        <div className="flex items-center justify-between gap-3 flex-wrap">
          <div className="flex items-center gap-3">
            <div className="w-11 h-11 rounded-xl bg-blue-50 flex items-center justify-center text-blue-600 border border-blue-100">
              <ShieldCheck className="w-6 h-6" />
            </div>
            <div>
              <h1 className="text-2xl font-bold text-slate-900">
                {lang === "zh" ? "关于 · 用户声明与免责条款" : "About · Statement & Disclaimer"}
              </h1>
              <span className="text-[11px] font-mono text-slate-400 tracking-widest uppercase">
                MedMe 医我
              </span>
            </div>
          </div>
          {/* 语言切换:默认中文,点 English 才显示英文 */}
          <div className="flex items-center rounded-lg border border-slate-200 bg-white overflow-hidden text-sm shrink-0">
            <button
              onClick={() => setLang("zh")}
              className={`px-3 py-1.5 cursor-pointer transition-colors ${
                lang === "zh" ? "bg-blue-600 text-white" : "text-slate-600 hover:bg-slate-50"
              }`}
            >
              中文
            </button>
            <button
              onClick={() => setLang("en")}
              className={`px-3 py-1.5 cursor-pointer transition-colors ${
                lang === "en" ? "bg-blue-600 text-white" : "text-slate-600 hover:bg-slate-50"
              }`}
            >
              English
            </button>
          </div>
        </div>

        {lang === "zh" && (
          <div className="text-sm text-slate-500 leading-relaxed">
            欢迎使用 MedMe(医我)。请在使用前仔细阅读以下声明。
          </div>
        )}

        <div className="space-y-4">
          {sections.map((s, i) => (
            <div key={i} className="bg-white rounded-2xl border border-slate-200 p-5 shadow-sm">
              <div className="text-slate-900 font-semibold mb-2">{s.t}</div>
              <p className="text-[15px] leading-relaxed text-slate-600">{s.b}</p>
            </div>
          ))}
        </div>

        <div className="text-xs font-mono text-slate-400 text-center">
          © MedMe Team 2026 ·{" "}
          <span
            onClick={onVersionClick}
            className="cursor-default select-none"
            title=""
          >
            v0.1
          </span>
        </div>
      </div>
    </div>
  );
}
