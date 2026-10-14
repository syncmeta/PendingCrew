# DeliveryValidation 原App独立 macOS 包工具

本单元只实现构建器/只读消费者源码及控制。没有新GUI/helper编译、签名、Keychain/App/后台运行、网络、设备、安装或发布。normal07、candidate13 与0f仍原来源；新工具不能给它们补标成持久验证产物。

build-crewd-delivery-validation-macos.py 默认 PLAN：完整40lowerhex --ref、明确正常后台/witness/profile、唯一私有新输出、原 warm DD/SPM，无 Xcode/签名/实际normal来源验收，计划明确字段尚未验证。自有两工具与复用的 inventory/provenance primitives 必须逐字匹配选择的 committed source；源配置/原入口/factory/scheme/marker检查且不采纳个人 Local。只有同时显式 --execute --adhoc 才能进入后续受审私有 artifact 构建；它不读取 Apple signing identity、不选择Team、不调用公证/发布/安装或启动App。这个后续动作尚未执行。

专用GUI配置是原target、PendingCrewDeliveryValidation scheme、DeliveryValidation Release型配置、PENDINGCREW_DELIVERY_VALIDATION条件、native-candidate ID、实际validation marker与完整sourceRef。实际settings必须无 DEBUG/NativeCandidate memory宏、Team/profile/entitlements，默认NOsign，Hardened YES，无preview/debugdylib。Info marker/ID/ref必须由实际build产生；工具不得事后修改Info掩盖配置错误。普通Release和旧NativeCandidate配置、源码、wire均不变。

正常后台在任何Xcode调用前要求准确compiledRef等于GUI完整ref、fresh true、arm64 Darwin/default/non-test/全部workspace和artifact features为空。随后由既有强reader核完整当前Rust inventory、raw Cargo/全部required owning graph/3sidecars/实际toolchain及明确debug或release。正常证据原件保留；仅 relocated witness evidenceDirectory改成backend-evidence，独立consumer核这一唯一变化。GUI Release不冒helper release编译；helper真实profile独立披露。

实际执行仅从完整Git archive取输入，用原DD/SPM，不建cold目录；锁定各SPM revision/洁净状态，完整checkouts/artifacts snapshot与外部inventory前后一致。容量至少2.5GiB。保存全Git/SPM/rawnormal、实际argv/settings/toolchain/编译日志和目标产物。实际 TARGET_BUILD_DIR/FULL_PRODUCT_NAME 必须在warm DD，签前完整GUI产品/entry保存；嵌入唯一verified normal后显式ad-hoc封口，无私钥身份。

消费者独立核每个Git blob/完整snapshot、SPM锁与完整snapshot/前后字节、完全相同实际settings/build请求及成功日志、toolchain/SDK、正常helper全部原始proof、原目标与签前GUI products、最终全部regular/link products。链接必须只到App内，不能外逃或引入特殊文件。全部Mach-O与CodeResources进入loaded-code清单，真实deepstrict复核；实际display必须是native-candidate/ad-hoc/noTeam/hardened，App entitlements空，保持不具publisher资格。它不执行bundle、不调用SecItem，不把封口签名当持久化或设备运行通过。

完整源控制覆盖默认PLAN/双显式build模式、坏ref/冷目录/个人Local/缺marker或错误factory、旧helper来源/feature扩大、配置Team/debug/entitlement差异、产品link外逃、实际命令/失败日志拒绝和source字段。真实完整编译/签名/消费者产物正例仍是未来唯一组合包动作；不能用受控配置或伪造产品证明该层。

Root先合并已接受恢复04和客户端/工具cut，backend产该完整ref唯一准确正常proof；包输入经QA接受后才安排必要完整Mac构建。移动验证通道入口已独立接受，真机/移动签名/容器与可达endpoint仍各自具体授权。后续fake持久服务必须先由受审真实后台给出签名authority/database/SPKI，再封唯一service/account scope；不得猜事实或绕过正式配对/TLS。每个App新PID、一般code admission与ownedQuit仍实际独立验收。
