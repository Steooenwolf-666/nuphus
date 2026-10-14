//! 循环执行子步骤
use super::*;

impl Executor {
    /// 循环执行子步骤
    pub(super) async fn execute_loop_step<F, Fut>(
        &self,
        step: &Step,
        def: &LoopDef,
        depth: u32,
        store: &WorkflowStore,
        events: &EventBus,
        tool_exec: &F,
        variables: &mut HashMap<String, serde_json::Value>,
        workflow_id: &str,
        llm: Option<&dyn ApiClient>,
        emitter: Option<&dyn EventEmitter>,
        tool_schemas: Option<&[ToolDefinition]>,
        completed_ids: &std::collections::HashSet<String>,
        run_record: &mut RunRecord,
    ) -> crate::Result<String>
    where
        F: Fn(String, serde_json::Value) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = std::result::Result<String, String>> + Send,
    {
        // ── ForEach ──
        if let Some(ref fe) = def.for_each {
            // B13：区分「空集合跳过（合法语义）」与「引用了不存在的变量（设计错误）」。
            // VarRef::Var 解析失败（resolve_var_by_path → None）说明变量名打错或
            // 未被先前步骤捕获——此时静默空转会掩盖笔误，发可观测警告；
            // 显式 Null / 空数组 / 空字符串等真实值仍按合法空集合处理。
            let (items_val, resolve_missed) = match &fe.items {
                crate::workflow::types::VarRef::Var { var } => {
                    match super::variables::resolve_var_by_path(var, variables) {
                        Some(v) => (v.clone(), false),
                        None => (serde_json::Value::Null, true),
                    }
                }
                crate::workflow::types::VarRef::Lit(s) => {
                    (serde_json::Value::String(s.clone()), false)
                }
            };
            if resolve_missed {
                let var_name = match &fe.items {
                    crate::workflow::types::VarRef::Var { var } => var.clone(),
                    crate::workflow::types::VarRef::Lit(s) => s.clone(),
                };
                let message = format!(
                    "Loop '{}' (for_each): 集合变量 '{{{{{}}}}}' 未解析到任何值（变量名打错或未被先前步骤捕获），循环跳过",
                    step.name, var_name
                );
                tracing::warn!("[executor] {message}");
                events.emit(WorkflowEvent::StepRunOutput {
                    step_id: step.id.clone(),
                    text: format!("[警告] {message}"),
                });
            }
            let items: Vec<serde_json::Value> = match items_val {
                serde_json::Value::Array(arr) => arr,
                serde_json::Value::String(s) => {
                    // Try parsing as JSON array
                    match serde_json::from_str::<Vec<serde_json::Value>>(&s) {
                        Ok(arr) => arr,
                        Err(_) => {
                            return Ok("loop_for_each_empty".to_string());
                        }
                    }
                }
                _ => {
                    return Ok("loop_for_each_empty".to_string());
                }
            };

            if items.is_empty() {
                return Ok("loop_for_each_empty".to_string());
            }

            for (i, item) in items.iter().enumerate() {
                self.check_cancel(workflow_id).await?;
                self.check_pause(
                    workflow_id,
                    events,
                    &step.id,
                    &step.name,
                    depth,
                    "loop",
                    Some(store),
                    Some(variables),
                )
                .await?;

                let mut scope = variables.clone();
                scope.insert(fe.item_var.clone(), item.clone());
                scope.insert("_index".to_string(), serde_json::Value::from(i as i64));

                for sub in &def.steps {
                    self.execute_step(
                        sub,
                        depth + 1,
                        store,
                        events,
                        tool_exec,
                        &mut scope,
                        workflow_id,
                        llm,
                        emitter,
                        tool_schemas,
                        completed_ids,
                        run_record,
                    )
                    .await?;

                    if sub.kind_str() == "break" {
                        return Ok("loop_broken".to_string());
                    }

                    if sub.kind_str() == "continue" {
                        break;
                    }
                }
                for (k, v) in &scope {
                    variables.insert(k.clone(), v.clone());
                }
            }
            return Ok("loop_completed".to_string());
        }

        // ── Fixed repeat ──
        if let Some(count) = def.repeat {
            for i in 0..count {
                self.check_cancel(workflow_id).await?;
                self.check_pause(
                    workflow_id,
                    events,
                    &step.id,
                    &step.name,
                    depth,
                    "loop",
                    Some(store),
                    Some(variables),
                )
                .await?;

                let mut scope = variables.clone();
                scope.insert("_index".to_string(), serde_json::Value::from(i as i64));

                for sub in &def.steps {
                    self.execute_step(
                        sub,
                        depth + 1,
                        store,
                        events,
                        tool_exec,
                        &mut scope,
                        workflow_id,
                        llm,
                        emitter,
                        tool_schemas,
                        completed_ids,
                        run_record,
                    )
                    .await?;

                    if sub.kind_str() == "break" {
                        return Ok("loop_broken".to_string());
                    }

                    if sub.kind_str() == "continue" {
                        break;
                    }
                }
                for (k, v) in &scope {
                    variables.insert(k.clone(), v.clone());
                }
            }
            return Ok("loop_completed".to_string());
        }

        // ── Until ──
        if let Some(ref until_cond) = def.until {
            let max = def.max;
            for _i in 0..max {
                self.check_cancel(workflow_id).await?;
                self.check_pause(
                    workflow_id,
                    events,
                    &step.id,
                    &step.name,
                    depth,
                    "loop",
                    Some(store),
                    Some(variables),
                )
                .await?;

                let mut scope = variables.clone();
                for sub in &def.steps {
                    self.execute_step(
                        sub,
                        depth + 1,
                        store,
                        events,
                        tool_exec,
                        &mut scope,
                        workflow_id,
                        llm,
                        emitter,
                        tool_schemas,
                        completed_ids,
                        run_record,
                    )
                    .await?;

                    if sub.kind_str() == "break" {
                        return Ok("loop_broken".to_string());
                    }

                    if sub.kind_str() == "continue" {
                        break;
                    }
                }
                for (k, v) in &scope {
                    variables.insert(k.clone(), v.clone());
                }

                // 检查终止条件
                crate::workflow::references::validate_until(until_cond, variables)
                    .map_err(crate::NuphusError::agent)?;
                if super::variables::eval_condition(until_cond, variables) {
                    return Ok("loop_until_met".to_string());
                }
            }
            // B14：until 跑满 max_iterations 仍未满足 → 静默返回 Ok 会掩盖
            // 「条件写错/永假」的设计错误。发可观测警告（StepRunOutput →
            // 前端 Inspector/timeline + tracing），运行结果本身保持 Ok——
            // 跑满不一定是错误（如轮询等待场景的兜底退出），只提醒不阻断。
            {
                let message = format!(
                    "Loop '{}' (until): 已达最大迭代次数 {} 但终止条件仍未满足，循环退出",
                    step.name, max
                );
                tracing::warn!("[executor] {message}");
                events.emit(WorkflowEvent::StepRunOutput {
                    step_id: step.id.clone(),
                    text: format!("[警告] {message}"),
                });
            }
            return Ok("loop_max_reached".to_string());
        }

        // No loop mode specified → single pass
        let mut scope = variables.clone();
        for sub in &def.steps {
            self.execute_step(
                sub,
                depth + 1,
                store,
                events,
                tool_exec,
                &mut scope,
                workflow_id,
                llm,
                emitter,
                tool_schemas,
                completed_ids,
                run_record,
            )
            .await?;

            if sub.kind_str() == "break" {
                return Ok("loop_broken".to_string());
            }

            if sub.kind_str() == "continue" {
                break;
            }
        }
        for (k, v) in &scope {
            variables.insert(k.clone(), v.clone());
        }
        Ok("loop_completed".to_string())
    }
}
