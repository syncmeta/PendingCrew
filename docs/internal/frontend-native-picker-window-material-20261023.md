# 原生工作区切换与整窗材质修正

基线：c678271e810ce668192295803996f326ffe0ee5f。用户 N158e 实际日用截图显示：五域仍为手写 Button 胶囊；灰侧栏与白色会话内容大面分块。此前 clear/source/编译证明不等于用户要求的实际玻璃外观。

## 完整净范围

- `CrewdGlassWorkspaceTabs` 改为系统 `Picker(.segmented)`；同一五域原绑定/业务页面保持挂载。删除手绘 selection、HStack buttons、叠加 glass 与横向自绘导航。短标签「文档」「进展」保持原域。它仍位于会话列表和正文共同上方，不横跨主 sidebar，不再加机组名称。
- Mac15+ 使用真正 `window` container 的系统 `.thinMaterial`；Mac14 降级保原同 scheme、幂等 AppKit representable，但采用公开 `.underWindowBackground`。不修改系统外观，不遍历私有 hosting 类。
- iOS18+ 各导航列使用透明 navigation container，让原单一 split 根 `.regularMaterial` 可见；crew List 隐藏默认 scroll 内容底色。iOS17 保原系统降级，不新建自绘背景。
- 不修改 sidebar 整高/固定新增＋、旧头像「人」footer、真实机器绿点、Unknown详情、原生 searchable、唯一电脑菜单、原文/事务/权限/恢复/退出语义。RootView 普通入口是 MacThreePaneView/IPadShell；备用 --crewd-ui 外观不作为当前 N 原图归因。

## 公开 API 依据

- https://developer.apple.com/documentation/swiftui/pickerstyle/segmented
- https://developer.apple.com/videos/play/wwdc2025/323/
- https://developer.apple.com/documentation/swiftui/containerbackgroundplacement/window
- https://developer.apple.com/documentation/swiftui/containerbackgroundplacement/navigation
- https://developer.apple.com/documentation/swiftui/containerbackgroundplacement/navigationsplitview

本机 SDK 已核 window placement macOS15+，navigation/split placement iOS18+；27 的 tabs 样式不引入第二分支。系统控件负责选中、键盘、辅助功能及新系统 Liquid Glass，源码不伪造其视觉。

## 验证与交付边界

原 aeh/SPM 暖缓存 Mac ordinary Debug LOCAL_MAC+REMOTE_PRODUCT 与 iOS ordinary Debug arm64 Simulator 全目标均 exit0；404 Swift/project/Package.resolved 输入前后相同。完整日志、argv、原驱动与公开 availability 材料在主仓 ignored `native-picker-window-material-validation-20261023`。仅用旧 N normal 作为明确历史 helper 编译输入；不是新准确候选、未签封/安装/启动或操作 GUI。

未改 Rust/API/schema/SPM/工程/版本，不重稳定业务或旧600秒。源净审通过后由根整合唯一完整输入，再由后台给新准确 normal，制作同源候选。最终实际宽/窄窗口须确认原生切换单行可达、材质整体呈现及文字对比；当前源码和目标编译不宣称视觉验收。若实际仍分块，继续本模块修复。
