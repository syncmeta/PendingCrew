import XCTest
// AgentModelCatalog.swift / AgentModelProbeParsers.swift 直接编进 PendingCrewTests target。

/// 可用模型表（Todo #37）+ 参数 fail-loud（Todo #36）的判定单测。
///
/// 覆盖任务书点名要的四件事：**表的解析**、**过期判定**、**过期时的降级表述**、
/// **fail-loud 判定**。
final class AgentModelCatalogTests: XCTestCase {

    private let iso = ISO8601DateFormatter()
    private func at(_ s: String) -> Date { ISO8601DateFormatter().date(from: s)! }

    // MARK: - claude 回显解析（实测原文，见 AgentModelProbeParsers 头注释）

    /// 2026-08-09 `claude -p "/model" --output-format json`（claude 2.1.226）的
    /// `result` 原文，一字未改。
    private let claudeModelEcho = """
        Current model: Opus 5 (effort: high)
        Usage: /model <name>. Available: sonnet, opus, haiku, fable, best, sonnet[1m], \
        opus[1m], fable[1m], opusplan, default, or a full model ID.
        """
    /// 同日 `claude -p "/effort" --output-format json` 的 `result` 原文。
    private let claudeEffortEcho = "Usage: /effort <low|medium|high|xhigh|max|ultracode|auto>"
    /// 同日 `claude --effort __pendingcrew_probe__ -p "/model" --output-format json` 的
    /// **stderr** 原文（启动参数那套只能从这句逼出来）。
    private let claudeLaunchEffortWarning =
        "Warning: Unknown --effort value '__pendingcrew_probe__' — ignoring it and using "
        + "the default effort. Valid values: low, medium, high, xhigh, max.\n"

    func testClaudeModelEchoParses() {
        let ids = ClaudeModelProbeParser.parseModels(claudeModelEcho)
        XCTAssertEqual(ids, ["sonnet", "opus", "haiku", "fable", "best",
                             "sonnet[1m]", "opus[1m]", "fable[1m]", "opusplan", "default"],
                       "「or a full model ID」是尾注不是别名，必须被滤掉；末尾句点也要去掉")
    }

    func testClaudeEffortEchoParses() {
        XCTAssertEqual(ClaudeModelProbeParser.parseEfforts(claudeEffortEcho),
                       ["low", "medium", "high", "xhigh", "max", "ultracode", "auto"])
    }

    func testClaudeCliJSONResultExtraction() {
        let stdout = #"{"is_error":false,"result":"Usage: /effort <low|high>","type":"result"}"#
        XCTAssertEqual(ClaudeModelProbeParser.resultField(fromJSON: stdout),
                       "Usage: /effort <low|high>")
        XCTAssertNil(ClaudeModelProbeParser.resultField(fromJSON: "not json"))
    }

    func testClaudeGarbageEchoYieldsNilNotEmptyTable() {
        // 解不出就回 nil 让调用方回落兜底表 —— 千万别回一张空表，
        // 空表 + 权威 = 把所有值都判成「不存在」。
        XCTAssertNil(ClaudeModelProbeParser.parseModels("Current model: Opus 5"))
        XCTAssertNil(ClaudeModelProbeParser.parseEfforts("no angle brackets here"))
        XCTAssertNil(ClaudeModelProbeParser.table(modelEcho: claudeModelEcho,
                                                  effortEcho: "garbage", probedAt: Date()),
                     "只解出一半也不许成表")
    }

    func testClaudeProbedTableShape() {
        let t = ClaudeModelProbeParser.table(modelEcho: claudeModelEcho,
                                             effortEcho: claudeEffortEcho,
                                             probedAt: at("2026-08-09T00:00:00Z"))
        let table = try! XCTUnwrap(t)
        XCTAssertEqual(table.agent, "claude")
        XCTAssertEqual(table.source, .probe)
        XCTAssertTrue(table.knowsModel("opus"))
        XCTAssertTrue(table.knowsModel("OPUS"), "大小写不敏感")
        XCTAssertFalse(table.knowsModel("gpt-5"))
        XCTAssertTrue(table.knowsEffort("ultracode"))
        // 长上下文变体合法但不占 picker。
        XCTAssertTrue(table.knowsModel("opus[1m]"))
        XCTAssertFalse(table.visibleModels.map(\.id).contains("opus[1m]"))
    }

    // MARK: - 启动态 vs 运行态 effort 是两套（2026-08-09 实测，本轮最要命的一条）

    func testLaunchEffortWarningParses() {
        XCTAssertEqual(ClaudeModelProbeParser.parseLaunchEfforts(fromWarning: claudeLaunchEffortWarning),
                       ["low", "medium", "high", "xhigh", "max"])
        XCTAssertNil(ClaudeModelProbeParser.parseLaunchEfforts(fromWarning: "没有 Valid values 那句"))
    }

    func testProbedTableSplitsLaunchAndRuntimeEfforts() throws {
        let table = try XCTUnwrap(ClaudeModelProbeParser.table(
            modelEcho: claudeModelEcho, effortEcho: claudeEffortEcho,
            launchEffortWarning: claudeLaunchEffortWarning, probedAt: at("2026-08-09T00:00:00Z")))
        XCTAssertEqual(table.efforts, ["low", "medium", "high", "xhigh", "max", "ultracode", "auto"])
        XCTAssertEqual(table.launchEfforts, ["low", "medium", "high", "xhigh", "max"])
        // ultracode：运行时有、启动帮助没写、实测启动也收 → 归未公开，不进 runtimeOnly。
        XCTAssertEqual(table.undocumentedLaunchEfforts, ["ultracode"])
        // ultracode 归「未公开但收」，**不算**只能运行时切 —— 否则同一个值会同时
        // 出现在「别拿去起 session」和「实测起 session 也收」两句里，自相矛盾。
        XCTAssertEqual(table.runtimeOnlyEfforts, ["auto"])
        XCTAssertTrue(table.knowsEffort("auto", phase: .runtime))
        XCTAssertFalse(table.knowsEffort("auto", phase: .launch),
                       "auto 只能运行时切 —— 起 session 传它会被静默降级")
        XCTAssertTrue(table.knowsEffort("ultracode", phase: .launch),
                      "帮助没写但实测收，别把它判成不可用")
        XCTAssertTrue(table.knowsEffort("high", phase: .launch))
    }

    func testMissingLaunchWarningFallsBackInsteadOfBreakingTable() throws {
        // 第三条探测失败不该废掉整张表 —— launchEfforts 留空 → 回落运行时那套。
        let table = try XCTUnwrap(ClaudeModelProbeParser.table(
            modelEcho: claudeModelEcho, effortEcho: claudeEffortEcho,
            launchEffortWarning: nil, probedAt: at("2026-08-09T00:00:00Z")))
        XCTAssertTrue(table.launchEfforts.isEmpty)
        XCTAssertEqual(table.effectiveLaunchEfforts, table.efforts)
        XCTAssertTrue(table.runtimeOnlyEfforts.isEmpty, "说不出差集时就别瞎报差集")
        XCTAssertTrue(table.knowsEffort("auto", phase: .launch), "没依据就别否定")
    }

    func testRuntimeOnlyEffortAtLaunchGetsItsOwnVerdict() throws {
        let table = try XCTUnwrap(ClaudeModelProbeParser.table(
            modelEcho: claudeModelEcho, effortEcho: claudeEffortEcho,
            launchEffortWarning: claudeLaunchEffortWarning, probedAt: Date()))
        let check = AgentModelValidator.checkEffort("auto", model: nil, table: table, phase: .launch)
        XCTAssertEqual(check,
                       .runtimeOnlyEffort(launchCandidates: ["low", "medium", "high", "xhigh", "max"]))
        let msg = try XCTUnwrap(AgentModelValidator.message(check, knob: "effort",
                                                            value: "auto", agent: "claude"))
        XCTAssertTrue(msg.contains("静默降级"), "必须点名这个坑，别笼统说「不在表里」：\(msg)")
        XCTAssertTrue(msg.contains("set_session_profile"), "要给出替代路径：\(msg)")
        // 同一个值在运行时那条腿上是完全合法的。
        XCTAssertEqual(AgentModelValidator.checkEffort("auto", model: nil, table: table,
                                                       phase: .runtime), .ok)
    }

    func testRuntimeOnlyVerdictSurvivesStaleTable() throws {
        // 「运行时有、启动没有」是探来的确定事实，不该因为表旧了就降级成含糊警告。
        var table = try XCTUnwrap(ClaudeModelProbeParser.table(
            modelEcho: claudeModelEcho, effortEcho: claudeEffortEcho,
            launchEffortWarning: claudeLaunchEffortWarning, probedAt: at("2026-07-01T00:00:00Z")))
        table.probedAt = "2026-07-01T00:00:00Z"
        let check = AgentModelValidator.checkEffort("auto", model: nil, table: table,
                                                    phase: .launch, now: at("2026-08-09T00:00:00Z"))
        guard case .runtimeOnlyEffort = check else {
            return XCTFail("陈旧表也该保住这条确定性结论：\(check)")
        }
    }

    func testSummaryLineSplitsTheTwoEffortSets() throws {
        let table = try XCTUnwrap(ClaudeModelProbeParser.table(
            modelEcho: claudeModelEcho, effortEcho: claudeEffortEcho,
            launchEffortWarning: claudeLaunchEffortWarning, probedAt: Date()))
        let line = AgentModelCatalog.summaryLine(for: table)
        XCTAssertTrue(line.contains("起 session 时的 effort"), line)
        XCTAssertTrue(line.contains("只能跑起来之后用 set_session_profile 切"),
                      "必须把「别拿去起 session」说出来：\(line)")
        XCTAssertTrue(line.contains("未公开"), "ultracode 要标不确定：\(line)")
    }

    func testFallbackClaudeTableAlsoCarriesTheSplit() {
        let t = AgentModelCatalog.claudeFallback
        XCTAssertEqual(t.launchEfforts, ["low", "medium", "high", "xhigh", "max"])
        XCTAssertEqual(t.runtimeOnlyEfforts, ["auto"])
        XCTAssertFalse(t.knowsEffort("auto", phase: .launch))
        // codex 两边同一套 —— 不该凭空造出差集。
        XCTAssertTrue(AgentModelCatalog.codexFallback.runtimeOnlyEfforts.isEmpty)
        XCTAssertTrue(AgentModelCatalog.codexFallback.knowsEffort("ultra", phase: .launch))
    }

    func testOldModelsJsonWithoutSplitStillDecodes() throws {
        // 旧盘上的 models.json 没有这两个字段 —— 必须还能解出来，别整份表退回兜底。
        let json = #"""
        {"agent":"claude","source":"probe","probedAt":"2026-08-09T00:00:00Z",
         "models":[{"id":"opus","efforts":[],"isDefault":false,"hidden":false}],
         "efforts":["low","high"]}
        """#
        let t = try JSONDecoder().decode(AgentModelTable.self, from: Data(json.utf8))
        XCTAssertTrue(t.launchEfforts.isEmpty)
        XCTAssertEqual(t.effectiveLaunchEfforts, ["low", "high"])
    }

    // MARK: - codex model/list 解析（实测响应帧节选）

    /// 2026-08-09 codex-cli 0.145.0 `model/list` 响应帧的 `result`（节选两个模型，
    /// 字段一字未改）。注意 gpt-5.5 **没有** max/ultra —— effort 逐模型不同。
    private var codexResult: [String: Any] {
        [
            "data": [
                ["id": "gpt-5.6-sol", "model": "gpt-5.6-sol", "displayName": "GPT-5.6-Sol",
                 "description": "Latest frontier agentic coding model.",
                 "hidden": false, "isDefault": true, "defaultReasoningEffort": "low",
                 "supportedReasoningEfforts": [
                    ["reasoningEffort": "low", "description": "Fast responses with lighter reasoning"],
                    ["reasoningEffort": "medium", "description": "Balances speed and reasoning depth"],
                    ["reasoningEffort": "high", "description": "Greater reasoning depth"],
                    ["reasoningEffort": "xhigh", "description": "Extra high reasoning depth"],
                    ["reasoningEffort": "max", "description": "Maximum reasoning depth"],
                    ["reasoningEffort": "ultra", "description": "Maximum reasoning with delegation"],
                 ]],
                ["id": "gpt-5.5", "model": "gpt-5.5", "displayName": "GPT-5.5",
                 "hidden": false, "isDefault": false, "defaultReasoningEffort": "medium",
                 "supportedReasoningEfforts": [
                    ["reasoningEffort": "low", "description": ""],
                    ["reasoningEffort": "medium", "description": ""],
                    ["reasoningEffort": "high", "description": ""],
                    ["reasoningEffort": "xhigh", "description": ""],
                 ]],
                ["id": "codex-auto-review", "model": "codex-auto-review",
                 "displayName": "Codex Auto Review", "hidden": true, "isDefault": false,
                 "defaultReasoningEffort": "medium", "supportedReasoningEfforts": []],
            ],
            "nextCursor": NSNull(),
        ]
    }

    func testCodexModelListParses() {
        let t = CodexModelProbeParser.table(result: codexResult, probedAt: at("2026-08-09T00:00:00Z"))
        let table = try! XCTUnwrap(t)
        XCTAssertEqual(table.agent, "codex")
        XCTAssertEqual(table.source, .probe)
        XCTAssertEqual(table.models.map(\.id), ["gpt-5.6-sol", "gpt-5.5", "codex-auto-review"])
        XCTAssertEqual(table.visibleModels.map(\.id), ["gpt-5.6-sol", "gpt-5.5"],
                       "hidden 的不进 picker，但仍是合法值")
        XCTAssertTrue(table.knowsModel("codex-auto-review"))
        XCTAssertEqual(table.efforts, ["low", "medium", "high", "xhigh", "max", "ultra"],
                       "表级 efforts = 并集，且保持低→高的出现次序")
    }

    func testCodexEffortIsPerModel() {
        let table = CodexModelProbeParser.table(result: codexResult,
                                                probedAt: at("2026-08-09T00:00:00Z"))!
        XCTAssertTrue(table.knowsEffort("ultra", forModel: "gpt-5.6-sol"))
        XCTAssertFalse(table.knowsEffort("ultra", forModel: "gpt-5.5"),
                       "gpt-5.5 不支持 ultra —— 按表级并集判会漏掉这个错")
        XCTAssertTrue(table.knowsEffort("ultra"), "不指定模型时退表级并集")
    }

    func testCodexMalformedResultYieldsNil() {
        XCTAssertNil(CodexModelProbeParser.table(result: nil, probedAt: Date()))
        XCTAssertNil(CodexModelProbeParser.table(result: ["data": []], probedAt: Date()))
        XCTAssertNil(CodexModelProbeParser.table(result: ["oops": 1], probedAt: Date()))
    }

    // MARK: - 过期判定

    private func probeTable(probedAt: String) -> AgentModelTable {
        AgentModelTable(agent: "claude", source: .probe, probedAt: probedAt,
                        models: [AgentModel(id: "opus")], efforts: ["high"])
    }

    private func manualTable(lastVerified: String?) -> AgentModelTable {
        AgentModelTable(agent: "codex", source: .manual, lastVerified: lastVerified,
                        models: [AgentModel(id: "gpt-5.5")], efforts: ["high"])
    }

    func testFreshProbeIsLive() {
        let now = at("2026-08-09T12:00:00Z")
        let t = probeTable(probedAt: "2026-08-09T06:00:00Z")
        XCTAssertEqual(AgentModelCatalog.freshness(of: t, now: now), .freshProbe)
        XCTAssertTrue(AgentModelCatalog.isLiveProbe(t, now: now))
        XCTAssertNil(AgentModelCatalog.stalenessNote(for: t, now: now),
                     "现探且新鲜 —— 不必给新鲜度提示")
    }

    func testStaleProbeIsNoLongerLive() {
        let now = at("2026-08-19T12:00:00Z")
        let t = probeTable(probedAt: "2026-08-09T06:00:00Z")
        XCTAssertEqual(AgentModelCatalog.freshness(of: t, now: now), .stale(days: 10))
        XCTAssertFalse(AgentModelCatalog.isLiveProbe(t, now: now),
                       "10 天前探的表已经不能自称「当前活表」")
    }

    func testManualTableIsNeverLive() {
        let now = at("2026-08-09T12:00:00Z")
        // 就算今天刚核实过，手工表也不是「现探的活表」。
        let t = manualTable(lastVerified: "2026-08-09")
        XCTAssertEqual(AgentModelCatalog.freshness(of: t, now: now), .manual(days: 0))
        XCTAssertFalse(AgentModelCatalog.isLiveProbe(t, now: now))
    }

    func testUndatedTableIsUndated() {
        let now = at("2026-08-09T12:00:00Z")
        XCTAssertEqual(AgentModelCatalog.freshness(of: manualTable(lastVerified: nil), now: now),
                       .undated)
        XCTAssertEqual(AgentModelCatalog.freshness(of: probeTable(probedAt: "不是时间"), now: now),
                       .undated)
    }

    func testFutureTimestampDoesNotProduceNegativeDays() {
        let now = at("2026-08-09T12:00:00Z")
        // 机器时钟跳变 / 时区乌龙时别输出「-3 天前核实」这种鬼话。
        XCTAssertEqual(AgentModelCatalog.freshness(of: manualTable(lastVerified: "2026-09-01"),
                                                   now: now), .manual(days: 0))
    }

    // MARK: - 过期时的降级表述（不许静默当事实呈现）

    func testStaleTableSaysHowManyDaysUnverified() {
        let now = at("2026-08-30T12:00:00Z")
        let note = AgentModelCatalog.stalenessNote(for: manualTable(lastVerified: "2026-08-09"),
                                                   now: now)
        let text = try! XCTUnwrap(note)
        XCTAssertTrue(text.contains("21 天"), "必须说出「X 天没核实过」的 X：\(text)")
        XCTAssertTrue(text.contains("可能已过时"), "必须明说可能过时：\(text)")
    }

    func testRecentManualTableStillDisclosesItIsManual() {
        let now = at("2026-08-10T12:00:00Z")
        let note = try! XCTUnwrap(
            AgentModelCatalog.stalenessNote(for: manualTable(lastVerified: "2026-08-09"), now: now))
        XCTAssertTrue(note.contains("手工"), "刚核实过也要说清这是手工兜底表，不是现探：\(note)")
    }

    func testUndatedTableSaysSo() {
        let note = try! XCTUnwrap(
            AgentModelCatalog.stalenessNote(for: manualTable(lastVerified: nil)))
        XCTAssertTrue(note.contains("可能已过时"))
    }

    func testSummaryLineCarriesStalenessInline() {
        let now = at("2026-08-30T12:00:00Z")
        let line = AgentModelCatalog.summaryLine(for: manualTable(lastVerified: "2026-08-09"),
                                                 now: now)
        XCTAssertTrue(line.contains("gpt-5.5"))
        XCTAssertTrue(line.contains("21 天"),
                      "清单与新鲜度提示必须是同一串 —— 分开返回迟早有人只取清单：\(line)")
    }

    func testFreshProbeSummaryHasNoWarning() {
        let now = at("2026-08-09T12:00:00Z")
        let line = AgentModelCatalog.summaryLine(for: probeTable(probedAt: "2026-08-09T06:00:00Z"),
                                                 now: now)
        XCTAssertFalse(line.contains("⚠️"), line)
    }

    // MARK: - fail-loud 判定（提示性，**永远不否决** —— 机长 2026-08-09 实测修正）

    /// 这条是整套东西的红线测试：`gpt-5-codex` 不在活表里，但实测证明后端仍解析得了、
    /// session 正常跑完。所以裁决只能是提示，**不许**出现任何「拒绝」语义。
    /// 谁把它改成白名单，这条会红。
    func testValueMissingFromLiveTableIsAdvisoryNotRejection() {
        let now = at("2026-08-09T12:00:00Z")
        let table = probeTable(probedAt: "2026-08-09T06:00:00Z")
        let check = AgentModelValidator.checkModel("gpt-5-codex", table: table, now: now)
        XCTAssertEqual(check, .notInLiveTable(candidates: ["opus"]))
        let msg = try! XCTUnwrap(AgentModelValidator.message(check, knob: "model",
                                                             value: "gpt-5-codex", agent: "claude"))
        XCTAssertTrue(msg.contains("gpt-5-codex"))
        XCTAssertTrue(msg.contains("opus"), "提示要带当代候选，让机长一眼能对照：\(msg)")
        XCTAssertTrue(msg.contains("没有拦你"), "必须写明「已放行」，别让读的人以为被拒了：\(msg)")
        XCTAssertFalse(msg.contains("ERROR"), "不是错误，别用错误措辞：\(msg)")
    }

    func testLiveTableAcceptsKnownValuesSilently() {
        let now = at("2026-08-09T12:00:00Z")
        let table = probeTable(probedAt: "2026-08-09T06:00:00Z")
        XCTAssertEqual(AgentModelValidator.checkModel("opus", table: table, now: now), .ok)
        XCTAssertEqual(AgentModelValidator.checkEffort("high", model: "opus", table: table, now: now),
                       .ok)
        XCTAssertFalse(AgentModelCheck.ok.needsAttention)
        XCTAssertNil(AgentModelValidator.message(.ok, knob: "model", value: "opus", agent: "claude"))
    }

    func testStaleTableCannotEvenClaimNotInLiveTable() {
        let now = at("2026-08-09T12:00:00Z")
        let table = manualTable(lastVerified: "2026-08-09")
        let check = AgentModelValidator.checkModel("gpt-6-brandnew", table: table, now: now)
        guard case let .unverifiable(_, candidates) = check else {
            return XCTFail("手工表连「不在活表里」都说不出口，只能声明没能对照：\(check)")
        }
        XCTAssertEqual(candidates, ["gpt-5.5"])
        XCTAssertTrue(check.needsAttention)
        let msg = try! XCTUnwrap(AgentModelValidator.message(check, knob: "model",
                                                             value: "gpt-6-brandnew", agent: "codex"))
        XCTAssertTrue(msg.contains("没能对照"), "放行也必须明说没对照过，不许静默：\(msg)")
        XCTAssertTrue(msg.contains("已照常执行"), msg)
    }

    func testNoTableAtAllIsUnverifiableNotOk() {
        let check = AgentModelValidator.checkModel("whatever", table: nil)
        guard case .unverifiable = check else { return XCTFail("无表时不许直接 ok：\(check)") }
    }

    func testEffortCheckUsesPerModelListWhenAvailable() {
        let table = AgentModelTable(
            agent: "codex", source: .probe, probedAt: ISO8601DateFormatter().string(from: Date()),
            models: [AgentModel(id: "gpt-5.5", efforts: ["low", "medium", "high", "xhigh"]),
                     AgentModel(id: "gpt-5.6-sol", efforts: ["low", "high", "ultra"])],
            efforts: ["low", "medium", "high", "xhigh", "ultra"])
        XCTAssertEqual(AgentModelValidator.checkEffort("ultra", model: "gpt-5.6-sol", table: table),
                       .ok)
        XCTAssertEqual(AgentModelValidator.checkEffort("ultra", model: "gpt-5.5", table: table),
                       .notInLiveTable(candidates: ["low", "medium", "high", "xhigh"]),
                       "候选要给**那个模型**的档位，不是表级并集")
    }

    // MARK: - 默认那条腿（picker 只管显式选；不选走 defaultModel 解析）

    func testSummaryLineDisclosesDefaultLeg() {
        let now = at("2026-08-09T12:00:00Z")
        var t = probeTable(probedAt: "2026-08-09T06:00:00Z")
        t.resolvedDefault = "gpt-5.6-sol"
        t.resolvedDefaultSource = "~/.codex/config.toml 顶层 model"
        let line = AgentModelCatalog.summaryLine(for: t, now: now)
        XCTAssertTrue(line.contains("不显式选 model 时实际跑 gpt-5.6-sol"), line)
        XCTAssertTrue(line.contains("config.toml"), "来源要说出来，否则机长没法自己去核：\(line)")
        XCTAssertTrue(line.contains("不是合法值白名单"), "清单语义要写死在同一串里：\(line)")
    }

    func testSummaryLineSaysSoWhenDefaultUnresolvable() {
        let line = AgentModelCatalog.summaryLine(for: probeTable(probedAt: "2026-08-09T06:00:00Z"),
                                                 now: at("2026-08-09T12:00:00Z"))
        XCTAssertTrue(line.contains("解析不出"), "解析不出就照实留白，不许瞎填一个：\(line)")
    }

    // MARK: - 落盘文件 / 兜底

    func testFileRoundTripAndResolve() throws {
        let file = AgentModelCatalogFile(
            claude: probeTable(probedAt: "2026-08-09T06:00:00Z"), codex: nil,
            codexError: "app-server 没答上")
        let data = try JSONEncoder().encode(file)
        let back = try JSONDecoder().decode(AgentModelCatalogFile.self, from: data)
        XCTAssertEqual(back, file)
        XCTAssertEqual(back.error(agent: "codex"), "app-server 没答上")
        // codex 这轮没探到 → 回落手工兜底表，不是「没表」。
        let codex = try XCTUnwrap(AgentModelCatalogFile.resolveTable(agent: "codex", file: back))
        XCTAssertEqual(codex.source, .manual)
        XCTAssertTrue(codex.knowsModel("gpt-5.6-sol"))
    }

    func testResolveWithNoFileFallsBackToManual() throws {
        let claude = try XCTUnwrap(AgentModelCatalogFile.resolveTable(agent: "claude", file: nil))
        XCTAssertEqual(claude.source, .manual)
        XCTAssertTrue(claude.knowsModel("opus"))
        XCTAssertNil(AgentModelCatalogFile.resolveTable(agent: "gemini", file: nil))
    }

    /// 兜底表是「当代该选哪个」的清单，所以不该再列 `gpt-5-codex` / `gpt-5` ——
    /// 它们在 codex-cli 0.145.0 的活表里已经不在了（2026-08-09 实测），旧 picker 把人
    /// 导向已下线的档。**注意这不等于说它们非法**（后端仍解析得了，见 `AgentModelCheck`），
    /// 只是不该继续推荐。
    func testFallbackTablesDropRetiredCodexAliases() {
        XCTAssertFalse(AgentModelCatalog.codexFallback.knowsModel("gpt-5-codex"))
        XCTAssertFalse(AgentModelCatalog.codexFallback.knowsModel("gpt-5"))
        XCTAssertTrue(AgentModelCatalog.codexFallback.knowsModel("gpt-5.6-sol"))
        XCTAssertTrue(AgentModelCatalog.claudeFallback.knowsModel("fable"))
        XCTAssertNotNil(AgentModelCatalog.parseDay(AgentModelCatalog.fallbackLastVerified),
                        "兜底表的核实日期必须可解析，否则新鲜度提示会退化成「无日期」")
    }

    // MARK: - picker 清单改读表（SessionLaunchOptions 不再硬编模型名）

    func testPickerListsComeFromProbedTable() {
        let probed = AgentModelCatalogFile(
            codex: AgentModelTable(
                agent: "codex", source: .probe,
                probedAt: ISO8601DateFormatter().string(from: Date()),
                models: [AgentModel(id: "gpt-9-future", efforts: ["low", "ultra"]),
                         AgentModel(id: "gpt-9-hidden", hidden: true)],
                efforts: ["low", "ultra"]))
        XCTAssertEqual(SessionLaunchOptions.models(for: .codex, catalog: probed), ["gpt-9-future"],
                       "picker 只列非 hidden 的现探候选")
        XCTAssertEqual(SessionLaunchOptions.efforts(for: .codex, catalog: probed), ["low", "ultra"])
    }

    func testPickerFallsBackToManualTableWithoutProbe() {
        // 没有 models.json 时也不能空 —— 回落手工兜底表。
        XCTAssertTrue(SessionLaunchOptions.models(for: .claudeCode, catalog: nil).contains("opus"))
        let codex = SessionLaunchOptions.models(for: .codex, catalog: nil)
        XCTAssertTrue(codex.contains("gpt-5.6-sol"))
        XCTAssertFalse(codex.contains("gpt-5-codex"), "picker 不该继续把人导向已下线的档")
        XCTAssertTrue(SessionLaunchOptions.efforts(for: .codex, catalog: nil).contains("xhigh"),
                      "旧硬编码的 minimal/low/medium/high 少了 xhigh/max/ultra")
    }

    func testDisplayNamePrefersTableName() {
        XCTAssertEqual(SessionLaunchOptions.displayName(for: "opus"), "Opus")
        XCTAssertEqual(SessionLaunchOptions.displayName(for: "gpt-5.6-sol"), "GPT-5.6-Sol",
                       "codex 侧的 displayName 来自表")
        XCTAssertEqual(SessionLaunchOptions.displayName(for: "某个没收录的值"), "某个没收录的值")
    }

    func testLoadFromDirectory() throws {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("model-catalog-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        XCTAssertNil(AgentModelCatalogFile.load(from: dir), "没文件就是 nil，不许崩")
        let file = AgentModelCatalogFile(claude: probeTable(probedAt: "2026-08-09T06:00:00Z"))
        try JSONEncoder().encode(file)
            .write(to: dir.appendingPathComponent(AgentModelCatalog.fileName))
        XCTAssertEqual(AgentModelCatalogFile.load(from: dir)?.claude?.models.map(\.id), ["opus"])
    }
}
