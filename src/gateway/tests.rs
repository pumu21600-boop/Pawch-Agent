    //! 客户端热重建机制验证：死代理 → 失败 → 环境恢复 + rebuild → 同一实例恢复连通。
    //! （bin crate 的 tests/ 目录拿不到内部符号，机制测试放这里）
    use super::*;

    fn init_logger() {
        static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let dir = std::env::temp_dir().join("agent-test-logs");
            let _ = crate::observability::init(&dir, "gw.jsonl", format!("gw-{}", uuid::Uuid::new_v4()));
        });
    }

    #[test]
    fn thinking_inflates_max_tokens_budget() {
        // 思考开启时上游把 reasoning+正文算进同一个 max_tokens，用户配的
        // 「最大输出长度」会被思考吃掉——发送值 = 配置值 + max(配置值, 8192)。
        // 思考关闭/未配 → 原值（此时上限本就只约束正文）。
        let mut p = ChatParams { max_tokens: Some(4096), thinking: Some("high".into()), ..Default::default() };
        assert_eq!(effective_max_tokens(&p), Some(12288));
        p.thinking = Some("on".into());
        assert_eq!(effective_max_tokens(&p), Some(12288));
        p.thinking = Some("off".into());
        assert_eq!(effective_max_tokens(&p), Some(4096), "思考关闭不扩容");
        p.thinking = None;
        assert_eq!(effective_max_tokens(&p), Some(4096), "未配思考档位不扩容");
        p.max_tokens = Some(12600);
        p.thinking = Some("high".into());
        assert_eq!(effective_max_tokens(&p), Some(25200), "配置值大于 8192 时按配置值翻倍");
        p.max_tokens = None;
        assert_eq!(effective_max_tokens(&p), None);
    }

    #[test]
    fn build_body_vendor_styles() {
        // REGISTRY 是进程级 OnceLock：测试进程里手动加载一次（幂等）
        let _ = crate::gateway::providers::init(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("providers.json"));
        // 预设收敛后旧预设 id 已下架：契约知识保留在端点探测，手工构造探测线索验证
        fn legacy(id: &str) -> ChatParams {
            ChatParams { provider_id: Some(id.into()), ..Default::default() }
        }
        let msgs = json!([{"role":"user","content":"hi"}]);
        // 智谱（zai 格式，端点探测出）：on/off 对象形态；GLM-5.3 起官方
        // reasoning_effort 三档 low/high/max（默认 max），显式档位折算后随 enabled 一起发
        let mut p = ChatParams::from_preset(crate::gateway::providers::find("zhipu").unwrap());
        p.thinking = Some("on".into());
        p.max_tokens = Some(2048);
        let b = build_body("glm-4.6", &msgs, true, &p, None);
        assert_eq!(b["thinking"], json!({"type":"enabled"}));
        assert_eq!(b["max_tokens"], json!(2048));
        assert_eq!(b["stream_options"]["include_usage"], json!(true));
        p.thinking = Some("off".into());
        let b = build_body("glm-4.6", &msgs, true, &p, None);
        assert_eq!(b["thinking"], json!({"type":"disabled"}));
        // 显式档位：low/high/max 原样发 effort；on 不发（渠道默认 max）
        p.thinking = Some("high".into());
        let b = build_body("glm-4.6", &msgs, true, &p, None);
        assert_eq!(b["thinking"], json!({"type":"enabled"}));
        assert_eq!(b["reasoning_effort"], json!("high"));
        for (lvl, want) in [("low", "low"), ("max", "max"), ("medium", "high"), ("xhigh", "max")] {
            p.thinking = Some(lvl.into());
            let b = build_body("glm-4.6", &msgs, true, &p, None);
            assert_eq!(b["reasoning_effort"], json!(want), "zai 档 {lvl} 应折算为 {want}");
        }
        p.thinking = Some("on".into());
        let b = build_body("glm-4.6", &msgs, true, &p, None);
        assert!(b.get("reasoning_effort").is_none(), "on 不发 effort=渠道默认");
        // 豆包（旧预设 id，契约知识保留在端点探测）：provider_id 直判 Zai
        let mut p = legacy("doubao");
        p.thinking = Some("on".into());
        let b = build_body("doubao-seed-1.6", &msgs, true, &p, None);
        assert_eq!(b["thinking"], json!({"type":"enabled"}));
        // 混元（旧预设 id）：provider_id 直判 qwen 形态，顶层布尔
        let mut p = legacy("hunyuan");
        p.thinking = Some("off".into());
        let b = build_body("hunyuan-turbos-latest", &msgs, false, &p, None);
        assert_eq!(b["enable_thinking"], json!(false));
        assert!(b.get("thinking").is_none(), "qwen 格式不发 thinking 对象");
        p.thinking = Some("on".into());
        let b = build_body("hunyuan-turbos-latest", &msgs, false, &p, None);
        assert_eq!(b["enable_thinking"], json!(true));
        // DeepSeek（deepseek 格式，端点探测）：thinking 对象；官方 reasoner 不收 effort
        let mut p = ChatParams::from_preset(crate::gateway::providers::find("deepseek").unwrap());
        p.thinking = Some("on".into());
        let b = build_body("deepseek-reasoner", &msgs, true, &p, None);
        assert_eq!(b["thinking"], json!({"type":"enabled"}));
        assert!(b.get("reasoning_effort").is_none(), "deepseek 格式 supports_reasoning_effort=false");
        // OpenAI（openai 格式）：on→medium；low/medium/high 原样；off 不发（o 系无可靠关参数）
        let mut p = legacy("openai");
        p.thinking = Some("on".into());
        let b = build_body("gpt-5", &msgs, true, &p, None);
        assert_eq!(b["reasoning_effort"], json!("medium"));
        for lvl in ["low", "medium", "high"] {
            p.thinking = Some(lvl.into());
            let b = build_body("gpt-5", &msgs, true, &p, None);
            assert_eq!(b["reasoning_effort"], json!(lvl), "档位 {lvl} 应解析后发出");
        }
        p.thinking = Some("xhigh".into());
        let b = build_body("gpt-5", &msgs, true, &p, None);
        assert_eq!(b["reasoning_effort"], json!("high"), "xhigh 在 gpt-5 上降级为 high");
        p.thinking = Some("off".into());
        let b = build_body("gpt-5", &msgs, true, &p, None);
        assert!(b.get("reasoning_effort").is_none(), "off 无参数可发时不发");
        // GPT-5 新系 max_tokens 字段切换
        let p = ChatParams { max_tokens: Some(1024), ..legacy("openai") };
        let b = build_body("gpt-5", &msgs, true, &p, None);
        assert!(b.get("max_tokens").is_none() && b["max_completion_tokens"] == json!(1024));
        let b = build_body("gpt-4o-mini", &msgs, true, &p, None);
        assert_eq!(b["max_tokens"], json!(1024));
        // 本地（qwen-chat-template）：chat_template_kwargs.enable_thinking
        let mut p = legacy("ollama");
        p.thinking = Some("on".into());
        let b = build_body("qwen3:8b", &msgs, true, &p, None);
        assert_eq!(b["chat_template_kwargs"]["enable_thinking"], json!(true));
        p.thinking = Some("off".into());
        let b = build_body("qwen3:8b", &msgs, true, &p, None);
        assert_eq!(b["chat_template_kwargs"]["enable_thinking"], json!(false));
        // OpenAI 兼容（自定义端点预设；端点认不出 → openai 格式）
        let mut p = ChatParams::from_preset(crate::gateway::providers::find("openai_compat").unwrap());
        p.thinking = Some("high".into());
        let b = build_body("mystery-model", &msgs, true, &p, None);
        assert_eq!(b["reasoning_effort"], json!("high"));
        // Anthropic 协议：build_body 入口直接产出 Messages 形状（system 顶层/max_tokens 必填/无 stream_options）
        let mut p = legacy("anthropic");
        p.thinking = Some("on".into());
        p.max_tokens = Some(8192);
        let b = build_body("claude-sonnet-4-5", &msgs, true, &p, None);
        assert_eq!(b["max_tokens"], json!(8192));
        assert!(b.get("stream_options").is_none(), "Anthropic 不带 OpenAI stream_options");
        assert_eq!(b["thinking"]["type"], json!("enabled"), "思考开 → thinking 对象（预算见 anthropic 模块测试）");
        // 默认不发任何思考参数（无论 None、空串、全空格，全网关均不产生任何思考字段）
        for empty in [None, Some("".to_string()), Some("   ".to_string())] {
            let mut p = ChatParams::from_preset(crate::gateway::providers::find("deepseek").unwrap());
            p.thinking = empty.clone();
            let b = build_body("deepseek-v4-flash", &msgs, true, &p, None);
            assert!(b.get("thinking").is_none() && b.get("reasoning_effort").is_none(), "DeepSeek 默认不注入参数");

            let mut p = legacy("qwen");
            p.thinking = empty.clone();
            let b = build_body("qwen3.8-flash", &msgs, true, &p, None);
            assert!(b.get("enable_thinking").is_none() && b.get("thinking").is_none(), "Qwen 默认不注入参数");

            let mut p = ChatParams::from_preset(crate::gateway::providers::find("zhipu").unwrap());
            p.thinking = empty.clone();
            let b = build_body("glm-5.2", &msgs, true, &p, None);
            assert!(b.get("thinking").is_none(), "Zai 默认不注入参数");
        }
        // 采样参数：有值才发
        let p = ChatParams { temperature: Some(0.3), top_p: Some(0.9), ..Default::default() };
        let b = build_body("m", &msgs, true, &p, None);
        assert_eq!(b["temperature"], json!(0.3));
        assert_eq!(b["top_p"], json!(0.9));
        let b = build_body("m", &msgs, true, &ChatParams::default(), None);
        assert!(b.get("temperature").is_none() && b.get("top_p").is_none());
        // 非流式不带 stream_options
        let b = build_body("m", &msgs, false, &ChatParams::default(), None);
        assert!(b.get("stream_options").is_none());
    }

    /// DeepSeek 思考档位映射（2026-10-31 依官方文档修正）：规范档只有
    /// none/low/high/max；medium/xhigh/minimal 是官方兼容折算值，本地先折成
    /// 规范档再发——此前把 max|xhigh 发成 "xhigh"、medium 原样外发都是错的。
    #[test]
    fn deepseek_effort_maps_to_canonical_low_high_max() {
        let msgs = json!([{ "role": "user", "content": "hi" }]);
        // REGISTRY 是进程级 OnceLock：测试进程里手动加载一次（幂等）
        let _ = crate::gateway::providers::init(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("providers.json"));
        let mut p = ChatParams::from_preset(crate::gateway::providers::find("deepseek").unwrap());
        for (req, sent) in [
            ("low", "low"),
            ("minimal", "low"),
            ("high", "high"),
            ("medium", "high"),
            ("xhigh", "high"),
            ("max", "max"),
        ] {
            p.thinking = Some(req.into());
            let b = build_body("deepseek-v4-flash", &msgs, true, &p, None);
            assert_eq!(b["thinking"], json!({"type":"enabled"}), "请求档 {req}");
            assert_eq!(b["reasoning_effort"], json!(sent), "请求档 {req} 应折算成官方规范档");
        }
        // on/未知档：thinking 开但不发 effort（官方默认 high）
        p.thinking = Some("on".into());
        let b = build_body("deepseek-v4-flash", &msgs, true, &p, None);
        assert_eq!(b["thinking"], json!({"type":"enabled"}));
        assert!(b.get("reasoning_effort").is_none());
        // off：thinking disabled + effort none（官方双关形态，thinking 对象优先）
        p.thinking = Some("off".into());
        let b = build_body("deepseek-v4-flash", &msgs, true, &p, None);
        assert_eq!(b["thinking"], json!({"type":"disabled"}));
        assert_eq!(b["reasoning_effort"], json!("none"));
    }

    // —— 用量台账：渠道标识与 usage 解析 ——

    #[test]
    fn host_of_strips_scheme_and_path_keeps_port() {
        assert_eq!(host_of("https://api.deepseek.com"), "api.deepseek.com");
        assert_eq!(host_of("https://api.deepseek.com/v1"), "api.deepseek.com");
        assert_eq!(host_of("http://127.0.0.1:9399/v1"), "127.0.0.1:9399");
        assert_eq!(host_of(" relay.example.com:8080 "), "relay.example.com:8080");
    }

    #[test]
    fn usage_triple_covers_openai_and_deepseek_styles() {
        // OpenAI 风格：prompt_tokens_details.cached_tokens
        let (p, c, k) = usage_triple(&json!({"usage": {"prompt_tokens": 100, "completion_tokens": 50, "prompt_tokens_details": {"cached_tokens": 64}}}));
        assert_eq!((p, c, k), (100, 50, 64));
        // DeepSeek 风格：prompt_cache_hit_tokens
        let (p, c, k) = usage_triple(&json!({"usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 8}}));
        assert_eq!((p, c, k), (10, 5, 8));
        // 渠道不回 usage：全 0（请求与延迟照记，见台账记录）
        assert_eq!(usage_triple(&json!({"choices": []})), (0, 0, 0));
        // output_tokens 兼容（个别反代用这个字段名）
        let (p, c, k) = usage_triple(&json!({"usage": {"prompt_tokens": 1, "output_tokens": 2}}));
        assert_eq!((p, c, k), (1, 2, 0));
        // 2026-10-08 修：Anthropic 的 input_tokens 不含 cache_read/cache_creation，
        // 而 OpenAI 的 prompt_tokens 是含缓存的总输入——补齐，否则锚点系统性低估
        let (p, c, k) = usage_triple(&json!({"usage": {
            "input_tokens": 100, "output_tokens": 50,
            "cache_read_input_tokens": 64, "cache_creation_input_tokens": 32,
        }}));
        assert_eq!((p, c, k), (196, 50, 64), "100+64+32；cached 只报 cache_read");
    }

    #[test]
    fn extract_delta_reasoning_fields() {
        // DeepSeek 式 reasoning_content
        let d = extract_delta(&json!({"choices":[{"delta":{"reasoning_content":"想"}}]}));
        assert_eq!(d.reasoning.as_deref(), Some("想"));
        // 兼容字段 reasoning / reasoning_text
        let d = extract_delta(&json!({"choices":[{"delta":{"reasoning":"R"}}]}));
        assert_eq!(d.reasoning.as_deref(), Some("R"));
        let d = extract_delta(&json!({"choices":[{"delta":{"reasoning_text":"T"}}]}));
        assert_eq!(d.reasoning.as_deref(), Some("T"));
        // OpenRouter reasoning_details
        let d = extract_delta(&json!({"choices":[{"delta":{"reasoning_details":[{"type":"reasoning.text","text":"OR"}]}}]}));
        assert_eq!(d.reasoning.as_deref(), Some("OR"));
        // 空串不触发
        let d = extract_delta(&json!({"choices":[{"delta":{"reasoning_content":""}}]}));
        assert_eq!(d.reasoning, None);
    }

    #[tokio::test]
    async fn throttle_enforces_interval() {
        init_logger();
        let t = Throttle::new(60); // 60 rpm → 最小间隔 1s
        let start = Instant::now();
        t.gate().await; // 第一发直通
        t.gate().await; // 第二发必须等满 1s
        let elapsed = start.elapsed();
        assert!(elapsed >= std::time::Duration::from_millis(900), "应被节流 ≥0.9s，实际 {elapsed:?}");
        assert!(elapsed < std::time::Duration::from_millis(2500), "不应过度等待，实际 {elapsed:?}");
    }

    #[test]
    fn think_split_variants() {
        // 无标签：原样走正文
        let mut ts = TagSplit::default();
        assert_eq!(ts.feed("普通回答"), ("".into(), "普通回答".into()));
        // 标签整段：思考与正文分离
        let mut ts = TagSplit::default();
        assert_eq!(ts.feed("<think>先想想</think>答案"), ("先想想".into(), "答案".into()));
        // 跨分片断裂："<thi|nk>AB</thi|nk>CD"
        let mut ts = TagSplit::default();
        let (t1, c1) = ts.feed("<thi");
        assert_eq!((t1.as_str(), c1.as_str()), ("", ""));
        let (t2, c2) = ts.feed("nk>AB</thi");
        assert_eq!((t2.as_str(), c2.as_str()), ("AB", ""));
        let (t3, c3) = ts.feed("nk>CD");
        assert_eq!((t3.as_str(), c3.as_str()), ("", "CD"));
        let (t4, c4) = ts.finish();
        assert_eq!((t4.as_str(), c4.as_str()), ("", ""));
        // 未闭合 <think>（流在思考中被截断）：全部走思考
        let mut ts = TagSplit::default();
        let (t, c) = ts.feed("<think>还没想完");
        assert_eq!((t.as_str(), c.as_str()), ("还没想完", ""));
        // 正文里出现 "<thinking" 这类伪前缀：不误伤（'a' 即时归正文，伪标签流尾归位）
        let mut ts = TagSplit::default();
        let (t, c) = ts.feed("a<thinking");
        assert_eq!((t.as_str(), c.as_str()), ("", "a"));
        let (t2, c2) = ts.finish();
        assert_eq!((t2.as_str(), c2.as_str()), ("", "<thinking"));
        // 孤立 </think>（不在思考态）：当普通文本
        let mut ts = TagSplit::default();
        assert_eq!(ts.feed("x</think>y"), ("".into(), "xy".into()));
        // 截断在半截标签：finish 归位正文
        let mut ts = TagSplit::default();
        let (t, c) = ts.feed("尾<thi");
        assert_eq!((t.as_str(), c.as_str()), ("", "尾"));
        let (t2, c2) = ts.finish();
        assert_eq!((t2.as_str(), c2.as_str()), ("", "<thi"));
    }

    #[tokio::test]
    async fn loopback_base_url_bypasses_proxy() {
        // 2026-10-08 修：环回渠道（本地模型服务 ollama/lmstudio/mnn/llamacpp 全是
        // 127.0.0.1 端点）原先会走系统/环境代理——代理连自己的 localhost 回 502，
        // 用户看到「渠道返回 502」而渠道本身没毛病。reqwest 默认不排除环回（注释里
        // 写了多年的「ProxyOverride 环回绕代理」从未实现），现在 Gateway 对环回
        // base_url 专用 no_proxy 客户端。本测试钉住：环境里挂着（死）代理时，
        // 环回请求照旧直达渠道。
        init_logger();
        let g = Gateway::new();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let mock = std::thread::spawn(move || {
            use std::io::Write;
            let resp = b"HTTP/1.1 401 Unauthorized
Content-Type: application/json
Content-Length: 2

{}";
            while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        // 监听器是非阻塞的，Windows 上接受的 socket 会继承该标志——
                        // 不恢复阻塞模式 write_all 会撞 WouldBlock（被 let _ 吞掉）
                        let _ = s.set_nonblocking(false);
                        // 先把请求读完再回响应：不读就关，未读的入站数据会让 Windows
                        // 直接回 RST（os error 10054），客户端只会看到「连接被重置」
                        let mut buf = [0u8; 4096];
                        let _ = std::io::Read::read(&mut s, &mut buf);
                        let _ = s.write_all(resp);
                        let _ = s.flush();
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        });
        let base = format!("http://127.0.0.1:{port}/v1");

        // 环境里挂一个死代理——环回请求必须绕过它直达 mock（401 = 网络层已通）
        unsafe {
            std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:1");
            std::env::set_var("HTTP_PROXY", "http://127.0.0.1:1");
        }
        g.rebuild_client_for_test();
        let r1 = g.list_models(&base, "sk-dummy").await;
        let msg1 = r1.err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(
            msg1.contains("401"),
            "环回 base_url 应绕过（死）代理直达渠道拿到 401，实际: {msg1}"
        );
        // 清掉代理再 rebuild：行为不变（环回客户端与代理配置无关）
        unsafe {
            std::env::remove_var("HTTPS_PROXY");
            std::env::remove_var("HTTP_PROXY");
        }
        g.rebuild_client_for_test();
        let r2 = g.list_models(&base, "sk-dummy").await;
        let msg2 = r2.err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(msg2.contains("401"), "清掉代理后仍应拿到 401，实际: {msg2}");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        mock.join().unwrap();
    }

    /// 环回主机判定：本地模型端点的各种写法都要认出来。
    #[test]
    fn loopback_base_url_detection() {
        for u in [
            "http://127.0.0.1:11434/v1",
            "http://localhost:1234/v1",
            "http://127.5.6.7:8080",
            "http://user:pass@127.0.0.1:8080/v1",
            "http://[::1]:8080/v1",
            "127.0.0.1:8080",
        ] {
            assert!(is_loopback_base_url(u), "{u} 应判为环回");
        }
        for u in [
            "https://api.deepseek.com/v1",
            "https://127.0.0.1.evil.com/v1",
            "https://open.bigmodel.cn/api/paas/v4",
            "https://2127.0.0.1/v1",
        ] {
            assert!(!is_loopback_base_url(u), "{u} 不应判为环回");
        }
    }
    #[test]
    fn take_line_survives_utf8_split_across_chunks() {
        // N1：网络分片边界不保证 UTF-8 字符完整。「你」= E4 BD A0、「好」= E5 A5 BD，
        // 从「你」第三个字节处切开——旧写法（逐分片 from_utf8_lossy）会把两个半截
        // 汉字碎成 U+FFFD 永久进正文。take_line 按字节攒完整行再解码，不该出替换符。
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice("data: {\"content\":\"\u{4f60}".as_bytes());
        buf.truncate(buf.len() - 1); // 砍掉「你」的最后一个字节，模拟分片切在字符中间
        assert_eq!(take_line(&mut buf), None, "半个汉字 + 半行不该出行");
        buf.extend_from_slice(b"\xA0"); // 补齐「你」被砍掉的尾字节（裸字节，不是 U+00A0 字符）
        buf.extend_from_slice("好".as_bytes());
        buf.extend_from_slice("\"}\n".as_bytes());
        let line = take_line(&mut buf).expect("补齐后应出完整行");
        assert_eq!(line, "data: {\"content\":\"你好\"}");
        assert!(!line.contains('\u{FFFD}'), "不该出现 U+FFFD 替换符: {line:?}");
        assert_eq!(take_line(&mut buf), None, "行已取完");
        // 回归：\r\n 行尾与空行照旧处理
        buf.extend_from_slice(b"data: x\r\n\r\n");
        assert_eq!(take_line(&mut buf).as_deref(), Some("data: x"));
        assert_eq!(take_line(&mut buf).as_deref(), Some(""));
    }

    /// 请求体摘要：必须丢掉 messages 正文（那是日志体积的真凶），但留住排查维度。
    ///
    /// 回归 2026-10-06 真机实测「时间线刷不出来」：完整 body 单条 2MB+，每步一条
    /// 就把 20MB 轮转上限撞穿，task.start 被归档删掉 → 时间线空白。摘要后单条
    /// 应只有几百字节。
    #[test]
    fn request_summary_drops_message_text_keeps_shape() {
        let body = json!({
            "model": "step-5-preview",
            "stream": true,
            "messages": [
                {"role": "system", "content": "x".repeat(20000)},
                {"role": "user", "content": "y".repeat(30000)},
            ],
            "tools": [{"type":"function"},{"type":"function"}],
        });
        let s = crate::gateway::request_summary(&body);
        let text = s.to_string();
        assert!(text.len() < 1000, "摘要必须远小于原体：{} 字节", text.len());
        assert!(!text.contains('x') || !text.contains("xxxxxxxxxx"), "正文不得进摘要");
        assert_eq!(s["model"], "step-5-preview");
        assert_eq!(s["messages"], 2);
        assert_eq!(s["message_chars"], 50000);
        assert_eq!(s["tools"], 2);
        // 每条消息的角色与字符数要留（定位「哪一轮把历史撑大了」靠它）
        assert_eq!(s["per_message"][0]["role"], "system");
        assert_eq!(s["per_message"][0]["chars"], 20000);
        assert_eq!(s["per_message"][1]["role"], "user");
    }

    /// prompt_cache_key：仅官方 OpenAI 端点发，且随会话 id 走（同会话前缀命中同一
    /// 缓存条目）；中转/国产端点一个都不发（普遍 400）；none 档连键也不发。
    #[test]
    fn prompt_cache_key_gating() {
        let msgs = json!([{"role":"user","content":"hi"}]);
        let compat = compat::resolve(Some("openai"), Some("https://api.openai.com/v1"), "gpt-5", None);
        let p = ChatParams::default();
        let b = build_body_with_compat("gpt-5", &msgs, true, &p, None, &compat, "sess_abc");
        assert_eq!(b["prompt_cache_key"], json!("sess_abc"), "官方端点 + short 档：带会话级缓存键");
        // 会话 id 缺省（辅助调用没归属）→ 不发键，不留 undefined 字段
        let b = build_body_with_compat("gpt-5", &msgs, true, &p, None, &compat, "  ");
        assert!(b.get("prompt_cache_key").is_none());
        // 中转端点：即便 retention=long 也不发（无 supports 标志）
        let relay = compat::resolve(Some("custom"), Some("https://relay.example.com/v1"), "gpt-5", None);
        let pl = ChatParams { cache_retention: CacheRetention::Long, ..Default::default() };
        let b = build_body_with_compat("gpt-5", &msgs, true, &pl, None, &relay, "sess_abc");
        assert!(b.get("prompt_cache_key").is_none(), "中转不发 prompt_cache_key");
        assert!(b.get("prompt_cache_retention").is_none(), "中转也不发 24h 保留");
        // none 档：官方也不发
        let pn = ChatParams { cache_retention: CacheRetention::None, ..Default::default() };
        let b = build_body_with_compat("gpt-5", &msgs, true, &pn, None, &compat, "sess_abc");
        assert!(b.get("prompt_cache_key").is_none(), "用户关缓存 → 键也不发");
        // long 档 + 官方：24h 保留 + 键
        let b = build_body_with_compat("gpt-5", &msgs, true, &pl, None, &compat, "sess_abc");
        assert_eq!(b["prompt_cache_retention"], json!("24h"));
        assert_eq!(b["prompt_cache_key"], json!("sess_abc"));
    }

    /// prompt_cache_key 截断到 64 字符（官方规范）；同会话内稳定即可，不求全局唯一。
    #[test]
    fn prompt_cache_key_clamped_to_64_chars() {
        let long = "s".repeat(100);
        assert_eq!(clamp_prompt_cache_key(&long).len(), 64);
        assert_eq!(clamp_prompt_cache_key("sess_abc"), "sess_abc");
        assert_eq!(clamp_prompt_cache_key("  sess_abc  "), "sess_abc", "trim 空白");
    }

    /// computer 工具截图回填的多模态 tool 消息：OpenAI 的 tool content 只收字符串，
    /// **模型没勾「图片解析」**时压平成一句人话（文本照拼、指路落盘截图）——
    /// 不硬发 400，也不假装模型看得见。
    #[test]
    fn tool_multimodal_flattened_when_model_has_no_vision() {
        let msgs = json!([
            { "role": "user", "content": "操作电脑" },
            { "role": "assistant", "content": null, "tool_calls": [
                { "id": "t1", "type": "function", "function": { "name": "computer", "arguments": "{}" } }
            ] },
            { "role": "tool", "tool_call_id": "t1", "content": [
                { "type": "text", "text": "已截图 1920×1080" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo" } }
            ] }
        ]);
        let compat = compat::resolve(Some("custom"), Some("https://relay.example.com/v1"), "m", None);
        let b = build_body_with_compat("m", &msgs, true, &ChatParams::default(), None, &compat, "s");
        let tool_msg = b["messages"].as_array().unwrap().iter().find(|m| m["role"] == json!("tool")).unwrap();
        let content = tool_msg["content"].as_str().expect("压平成字符串");
        assert!(content.contains("已截图 1920×1080"), "文本照拼: {content}");
        assert!(content.contains("没勾「图片解析」"), "图片降级成人话: {content}");
        assert!(content.contains("screenshots/"), "指路落盘目录: {content}");
        assert!(!content.contains("iVBORw0KGgo"), "base64 绝不上链路");
        assert_eq!(b["messages"].as_array().unwrap().len(), 3, "没有能力时不许凭空空插 user 消息");
        // 非 tool 消息与纯字符串 tool 消息不受影响
        let plain = json!([
            { "role": "user", "content": "hi" },
            { "role": "tool", "tool_call_id": "t0", "content": "纯文本结果" }
        ]);
        let b2 = build_body_with_compat("m", &plain, true, &ChatParams::default(), None, &compat, "s");
        assert_eq!(b2["messages"][1]["content"], json!("纯文本结果"));
    }

    /// 模型**勾了「图片解析」**时，截图不能丢：OpenAI 的 tool content 塞不下图，
    /// 图要挪进紧随其后的一条 user 消息（和用户发附件的形状完全一样）。
    /// 连续多条 tool 消息时只补一条 user（OpenAI 要求 tool 消息连续给完）。
    #[test]
    fn tool_multimodal_delivered_as_user_message_when_vision_on() {
        let msgs = json!([
            { "role": "user", "content": "操作电脑" },
            { "role": "assistant", "content": null, "tool_calls": [
                { "id": "t1", "type": "function", "function": { "name": "computer", "arguments": "{}" } },
                { "id": "t2", "type": "function", "function": { "name": "computer", "arguments": "{}" } }
            ] },
            { "role": "tool", "tool_call_id": "t1", "content": [
                { "type": "text", "text": "已截图 1920×1080" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo" } }
            ] },
            { "role": "tool", "tool_call_id": "t2", "content": "纯文本结果" }
        ]);
        let compat = compat::resolve(Some("custom"), Some("https://relay.example.com/v1"), "m", None);
        let p = ChatParams { vision: true, ..Default::default() };
        let b = build_body_with_compat("m", &msgs, true, &p, None, &compat, "s");
        let arr = b["messages"].as_array().unwrap();
        assert_eq!(arr.len(), 5, "4 条原始消息 + 1 条补图 user: {arr:?}");
        assert_eq!(arr[0]["role"], json!("user"));
        assert_eq!(arr[1]["role"], json!("assistant"));
        assert_eq!(arr[2]["role"], json!("tool"), "tool 仍连续，中间不许插 user");
        assert_eq!(arr[3]["role"], json!("tool"));
        let tool_text = arr[2]["content"].as_str().expect("tool content 必须是字符串");
        assert!(tool_text.contains("已截图 1920×1080"));
        assert!(tool_text.contains("截图已附在下一条消息里"), "{tool_text}");
        assert!(!tool_text.contains("iVBORw0KGgo"), "base64 不进 tool 文本");
        assert_eq!(arr[4]["role"], json!("user"), "截图落在连续 tool 组之后");
        let parts = arr[4]["content"].as_array().expect("user 多模态数组");
        assert!(parts.iter().any(|p| p["type"] == json!("image_url")), "图必须在");
        assert!(parts.iter().any(|p| p["type"] == json!("text")), "要带一句说明");
        assert!(
            arr[4]["content"].to_string().contains("iVBORw0KGgo"),
            "图挪到了 user 消息里"
        );
        // 纯字符串 tool 消息 + 无图：不补 user 消息
        let plain = json!([
            { "role": "user", "content": "hi" },
            { "role": "tool", "tool_call_id": "t0", "content": "纯文本结果" }
        ]);
        let b2 = build_body_with_compat("m", &plain, true, &p, None, &compat, "s");
        assert_eq!(b2["messages"].as_array().unwrap().len(), 2);
    }

    /// 拟真渠道的两种「僵住」形态（2026-10-20 实测 bug 1/2 的靶子）：
    /// - `HeadersOnly`：回了响应头但一个分片都不吐（渠道静默）；
    /// - `NeverRespond`：连响应头都不回（渠道僵死）。
    ///
    /// 都保持连接不关，模拟真实渠道卡住而不是断开。
    #[derive(Clone, Copy)]
    enum MockMode {
        HeadersOnly,
        NeverRespond,
    }

    fn mock_stalled_provider(mode: MockMode) -> (String, Arc<std::sync::atomic::AtomicBool>, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::spawn(move || {
            while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        // 监听器非阻塞，接受的 socket 会继承该标志（同 loopback 测试的坑）
                        let _ = s.set_nonblocking(false);
                        // 先把请求读掉再僵住：不读就挂会让对端写阻塞，测的就不是超时路径了
                        let mut buf = [0u8; 8192];
                        let _ = std::io::Read::read(&mut s, &mut buf);
                        if let MockMode::HeadersOnly = mode {
                            use std::io::Write;
                            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n");
                            let _ = s.flush();
                        }
                        while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        });
        (format!("http://127.0.0.1:{port}/v1"), stop, handle)
    }

    /// 跑一次会被「停止」打断的对话：400ms 后置取消标志，断言 5s 内返回 Cancelled。
    async fn assert_cancel_interrupts(mode: MockMode) {
        init_logger();
        let (base, stop, handle) = mock_stalled_provider(mode);
        let gw = Arc::new(Gateway::new());
        let ch = Channel::new(&base, "m", "sk-test", ChatParams::default());
        let cancel = Arc::new(CancelFlag::new());
        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            cancel2.set();
        });
        let started = Instant::now();
        let out = gw
            .stream_chat(
                &UsageCtx::main("sess-cancel"),
                &ch,
                &json!([{"role":"user","content":"hi"}]),
                None,
                &cancel,
                None,
                |_| {},
            )
            .await;
        let elapsed = started.elapsed();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        handle.join().unwrap();
        match out {
            Ok(ChatOutcome::Cancelled) => {}
            Ok(ChatOutcome::Done(_)) => panic!("应返回 Cancelled，实际拿到完整回答"),
            Ok(ChatOutcome::Interrupted(_)) => panic!("应返回 Cancelled，实际拿到打断"),
            Err(e) => panic!("应返回 Cancelled，实际报错: {e:#}"),
        }
        // 旧实现要干等到 120s 空闲超时（静默）或 600s 客户端超时（无响应）
        assert!(elapsed < std::time::Duration::from_secs(5), "停止必须在秒级生效，实际 {elapsed:?}");
    }

    /// bug 1 回归：渠道回了响应头却一直不吐分片时，点「停止」必须当场生效。
    #[tokio::test]
    async fn cancel_interrupts_silent_stream() {
        assert_cancel_interrupts(MockMode::HeadersOnly).await;
    }

    /// bug 2 回归：渠道连响应头都不回（僵死）时，点「停止」同样必须当场生效。
    #[tokio::test]
    async fn cancel_interrupts_hung_request() {
        assert_cancel_interrupts(MockMode::NeverRespond).await;
    }

    /// 2026-11-01 回归：读流失败的消息必须带出**整条**错误链。
    /// reqwest 0.12 的 Display 即使 `{:#}` 也只打「error decoding response body」，
    /// 连接重置/提前 EOF 这些真因只在 source() 里——断流排查全指望 error_chain。
    #[test]
    fn error_chain_walks_sources() {
        #[derive(Debug)]
        struct Layer(&'static str, Option<Box<Layer>>);
        impl std::fmt::Display for Layer {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for Layer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.1.as_ref().map(|b| b.as_ref() as &(dyn std::error::Error + 'static))
            }
        }
        let e = Layer(
            "error decoding response body",
            Some(Box::new(Layer(
                "error reading a body from connection",
                Some(Box::new(Layer("connection reset by peer (os error 10054)", None))),
            ))),
        );
        let s = error_chain(&e);
        assert!(s.contains("error decoding response body"), "{s}");
        assert!(s.contains("error reading a body from connection"), "底层原因必须带出来: {s}");
        assert!(s.contains("connection reset by peer"), "最底层原因也要带出来: {s}");
        assert_eq!(s.matches(" <- ").count(), 2, "两层 cause，不多不少: {s}");
        // 单层错误（没有 source）原样返回
        let solo = Layer("模型流超时：120 秒无新数据", None);
        assert_eq!(error_chain(&solo), "模型流超时：120 秒无新数据");
    }

    #[test]
    fn thinking_degraded_notice_only_once_per_channel() {
        let gw = Gateway::new();
        assert!(!gw.take_thinking_degraded_notice("https://a", "m"), "未降级不提示");
        gw.mark_thinking_degraded("https://a", "m");
        assert!(gw.take_thinking_degraded_notice("https://a", "m"), "首次命中要提示");
        assert!(!gw.take_thinking_degraded_notice("https://a", "m"), "同渠道第二次不再提示");
        assert!(gw.thinking_is_degraded("https://a", "m"), "降级登记本身还在（剥参逻辑照旧）");
        gw.mark_thinking_degraded("https://b", "m");
        assert!(gw.take_thinking_degraded_notice("https://b", "m"), "不同渠道各自提示一次");
    }

    #[test]
    fn max_tokens_inflate_rejected_is_per_channel() {
        let gw = Gateway::new();
        assert!(!gw.max_tokens_inflate_rejected("https://a", "m"), "未拒收不跳过扩容");
        gw.mark_max_tokens_inflate_rejected("https://a", "m");
        assert!(gw.max_tokens_inflate_rejected("https://a", "m"), "登记后跳过扩容");
        assert!(!gw.max_tokens_inflate_rejected("https://b", "m"), "不同渠道互不影响");
        assert!(!gw.thinking_is_degraded("https://a", "m"), "扩容被拒不能登记成拒收思考");
    }

    /// 2026-11-05 回归：400「max_tokens 超限」不能被误判成「端点拒收思考参数」。
    /// 旧顺序先剥参重试（去掉 thinking 后 max_tokens 恰好回到配置原值，于是成功），
    /// 把渠道永久登记成 thinking_degraded——思考被关掉，每次重启还弹一次通知。
    /// 正确行为：先用「配置原值 + 思考」重试，成功则保住思考；后续请求不再扩容。
    /// 读一个完整 HTTP 请求（headers + Content-Length 指定的 body），测试 mock 用。
    fn read_http_request(s: &mut std::net::TcpStream) -> String {
        use std::io::Read;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let n = s.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                let len = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        if k.eq_ignore_ascii_case("content-length") {
                            v.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                if buf.len() >= pos + 4 + len {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    #[tokio::test]
    async fn max_tokens_400_keeps_thinking() {
        use std::io::Write;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let bodies = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let bodies2 = bodies.clone();
        let handle = std::thread::spawn(move || {
            for i in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let _ = s.set_nonblocking(false);
                let raw = read_http_request(&mut s);
                if let Some(body) = raw.split("\r\n\r\n").nth(1)
                    && let Ok(v) = serde_json::from_str::<Value>(body)
                {
                    bodies2.lock().unwrap().push(v);
                }
                if i == 0 {
                    // 与 deepseek 实际返回同形：拒的是 max_tokens 超限，不是思考字段
                    let err = r#"{"error":{"message":"Invalid max_tokens value, the valid range of max_tokens is [1, 393216]","type":"invalid_request_error"}}"#;
                    let resp = format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        err.len(),
                        err
                    );
                    let _ = s.write_all(resp.as_bytes());
                } else {
                    let chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        chunk.len(),
                        chunk
                    );
                    let _ = s.write_all(resp.as_bytes());
                }
                let _ = s.flush();
            }
        });

        init_logger();
        let base = format!("http://127.0.0.1:{port}/v1");
        let gw = Arc::new(Gateway::new());
        let params = ChatParams {
            provider_id: Some("deepseek".into()),
            max_tokens: Some(384000),
            thinking: Some("max".into()),
            ..Default::default()
        };
        let ch = Channel::new(&base, "deepseek-flash", "sk-test", params);
        let cancel = Arc::new(CancelFlag::new());
        let out = gw
            .stream_chat(
                &UsageCtx::main("sess-max-tokens"),
                &ch,
                &json!([{"role":"user","content":"hi"}]),
                None,
                &cancel,
                None,
                |_| {},
            )
            .await;
        handle.join().unwrap();

        let captured = bodies.lock().unwrap().clone();
        assert_eq!(captured.len(), 2, "应有一次 400 回退重试: {captured:?}");
        assert_eq!(captured[0]["max_tokens"].as_u64(), Some(768000), "首请求是扩容值");
        assert_eq!(captured[1]["max_tokens"].as_u64(), Some(384000), "回退重试发配置原值");
        assert!(
            Gateway::body_has_thinking_params(&captured[1]),
            "回退重试必须保留思考参数: {}",
            captured[1]
        );
        assert!(matches!(out, Ok(ChatOutcome::Done(_))), "回退后应成功");
        assert!(!gw.thinking_is_degraded(&base, "deepseek-flash"), "不能误登记成端点拒收思考");
        assert!(gw.max_tokens_inflate_rejected(&base, "deepseek-flash"), "应记住该渠道不再扩容");
    }

    /// 2026-11-05 回归：渠道偶发 502 时最多重试 5 次（退避 1s/2s/3s/4s/5s），
    /// 第 6 次成功则整轮照常继续；重试请求体保持一致，不偷偷改参数。
    #[tokio::test]
    async fn server_error_retries_five_times_then_succeeds() {
        use std::io::Write;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let bodies = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let bodies2 = bodies.clone();
        let handle = std::thread::spawn(move || {
            for i in 0..6 {
                let (mut s, _) = listener.accept().unwrap();
                let _ = s.set_nonblocking(false);
                let raw = read_http_request(&mut s);
                if let Some(body) = raw.split("\r\n\r\n").nth(1)
                    && let Ok(v) = serde_json::from_str::<Value>(body)
                {
                    bodies2.lock().unwrap().push(v);
                }
                if i < 5 {
                    let body = "error code: 502\n";
                    let resp = format!(
                        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = s.write_all(resp.as_bytes());
                } else {
                    let chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        chunk.len(),
                        chunk
                    );
                    let _ = s.write_all(resp.as_bytes());
                }
                let _ = s.flush();
            }
        });

        init_logger();
        let base = format!("http://127.0.0.1:{port}/v1");
        let gw = Arc::new(Gateway::new());
        let params = ChatParams {
            provider_id: Some("deepseek".into()),
            max_tokens: Some(384000),
            thinking: Some("max".into()),
            ..Default::default()
        };
        let ch = Channel::new(&base, "deepseek-flash", "sk-test", params);
        let cancel = Arc::new(CancelFlag::new());
        let started = Instant::now();
        let out = gw
            .stream_chat(
                &UsageCtx::main("sess-5xx"),
                &ch,
                &json!([{"role":"user","content":"hi"}]),
                None,
                &cancel,
                None,
                |_| {},
            )
            .await;
        handle.join().unwrap();

        let captured = bodies.lock().unwrap().clone();
        assert_eq!(captured.len(), 6, "1 次原请求 + 5 次重试: {captured:?}");
        assert!(captured.iter().all(|b| b["max_tokens"].as_u64() == Some(768000)), "重试体保持一致");
        assert!(matches!(out, Ok(ChatOutcome::Done(_))), "第 6 次成功后应正常完成");
        assert!(started.elapsed() >= std::time::Duration::from_secs(15), "5 次退避应累计 15s");
    }
