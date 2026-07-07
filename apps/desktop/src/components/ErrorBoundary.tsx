import { Component, type ErrorInfo, type ReactNode } from "react";
import { AlertTriangle, RotateCcw } from "lucide-react";

// 顶层错误边界:渲染/生命周期阶段的同步抛错(最容易触发的是 dwv 的
// app.init/loadImageObject,见 DicomViewer.tsx)否则会把整个应用白屏。
// 捕获后展示一个可恢复的简单提示,而不是让用户面对空白页面。

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
}

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { hasError: false };

  static getDerivedStateFromError(): State {
    return { hasError: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("[ErrorBoundary] 捕获到渲染错误", error, info);
  }

  handleReload = () => {
    this.setState({ hasError: false });
    window.location.reload();
  };

  render() {
    if (this.state.hasError) {
      return (
        <div className="flex-1 h-full w-full flex flex-col items-center justify-center gap-4 bg-slate-50 text-center px-6">
          <div className="w-12 h-12 rounded-full bg-rose-50 text-rose-600 flex items-center justify-center">
            <AlertTriangle className="w-6 h-6" />
          </div>
          <div className="text-slate-700 font-medium">出了点问题,请重试</div>
          <button
            onClick={this.handleReload}
            className="flex items-center gap-1.5 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 rounded-xl px-4 py-2 transition-colors cursor-pointer"
          >
            <RotateCcw className="w-4 h-4" /> 重新加载
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
