use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rquickjs::prelude::Async;
use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Function, Object, Persistent, Value,
};

use super::Outcome;
use super::host::Host;

const PRELUDE: &str = include_str!("script/prelude.js");
const HEAP_BYTES: usize = 32 * 1024 * 1024;
const BUSY_LIMIT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 30 });

pub(crate) struct Script {
    finish: Persistent<Function<'static>>,
    context: AsyncContext,
    runtime: AsyncRuntime,
    busy: Rc<Busy>,
}

struct Busy {
    since: Cell<Instant>,
    tripped: Cell<bool>,
}

impl Busy {
    fn new() -> Self {
        Self {
            since: Cell::new(Instant::now()),
            tripped: Cell::new(false),
        }
    }

    fn rest(&self) {
        self.since.set(Instant::now());
    }

    fn is_over(&self) -> bool {
        let over = self.since.get().elapsed() > BUSY_LIMIT;
        if over {
            self.tripped.set(true);
        }
        over
    }
}

impl Script {
    pub(crate) async fn new(host: Rc<Host>) -> rquickjs::Result<Self> {
        let runtime = AsyncRuntime::new()?;
        runtime.set_memory_limit(HEAP_BYTES).await;
        let busy = Rc::new(Busy::new());
        let watch = Rc::clone(&busy);
        runtime
            .set_interrupt_handler(Some(Box::new(move || watch.is_over())))
            .await;
        let context = AsyncContext::full(&runtime).await?;
        let pulse = Rc::clone(&busy);
        let finish = context
            .with(|ctx| {
                let call = Function::new(
                    ctx.clone(),
                    Async(move |request: String| {
                        let host = Rc::clone(&host);
                        let pulse = Rc::clone(&pulse);
                        async move {
                            let reply = host.call(&request).await;
                            pulse.rest();
                            rquickjs::Result::Ok(reply)
                        }
                    }),
                )?;
                let prelude: Function = ctx.eval(PRELUDE)?;
                let finish: Function = prelude.call((call,))?;
                Ok::<_, rquickjs::Error>(Persistent::save(&ctx, finish))
            })
            .await?;
        Ok(Self {
            finish,
            context,
            runtime,
            busy,
        })
    }

    pub(crate) async fn eval(&self, code: &str) -> Outcome {
        self.busy.rest();
        self.busy.tripped.set(false);
        let finish = self.finish.clone();
        let busy = Rc::clone(&self.busy);
        let outcome = self
            .context
            .async_with(async |ctx| {
                let Ok(finish) = finish.restore(&ctx) else {
                    return Outcome::error("the sandbox lost its output function");
                };
                match evaluate(&ctx, code).await {
                    Ok(value) => Outcome::ok(render(&ctx, &finish, value)),
                    Err(problem) => {
                        let logged = render(&ctx, &finish, Value::new_undefined(ctx.clone()));
                        let problem = if busy.tripped.get() {
                            format!(
                                "stopped: the script ran JavaScript for {}s without awaiting a call",
                                BUSY_LIMIT.as_secs()
                            )
                        } else {
                            problem
                        };
                        Outcome::error(join(&logged, &problem))
                    }
                }
            })
            .await;
        self.runtime.idle().await;
        outcome
    }
}

async fn evaluate<'js>(ctx: &Ctx<'js>, code: &str) -> Result<Value<'js>, String> {
    let promise = ctx
        .eval_promise(code)
        .catch(ctx)
        .map_err(|e| e.to_string())?;
    let completion: Object = promise
        .into_future()
        .await
        .catch(ctx)
        .map_err(|e| e.to_string())?;
    completion
        .get("value")
        .catch(ctx)
        .map_err(|e| e.to_string())
}

fn render<'js>(ctx: &Ctx<'js>, finish: &Function<'js>, value: Value<'js>) -> String {
    finish
        .call::<_, String>((value,))
        .catch(ctx)
        .unwrap_or_else(|err| format!("the result could not be shown: {err}"))
}

fn join(logged: &str, problem: &str) -> String {
    if logged.is_empty() {
        problem.to_owned()
    } else {
        format!("{logged}\n{problem}")
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tokio::sync::mpsc;

    use super::*;

    async fn script(dir: &Path) -> Script {
        let (tx, _) = mpsc::channel(1);
        let host = Host::new(dir, dir, &dir.join(".spill"), tx).unwrap();
        Script::new(Rc::new(host)).await.unwrap()
    }

    #[tokio::test]
    async fn last_expression_is_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = script(dir.path()).await.eval("const a = 2; a * 21").await;
        assert_eq!(outcome, Outcome::ok("42"));
    }

    #[tokio::test]
    async fn variables_persist_between_runs() {
        let dir = tempfile::tempdir().unwrap();
        let script = script(dir.path()).await;
        script.eval("let seen = ['a']").await;
        assert_eq!(
            script.eval("seen.concat('b')").await.output.as_ref(),
            "[\n  \"a\",\n  \"b\"\n]"
        );
    }

    #[tokio::test]
    async fn logs_come_before_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = script(dir.path())
            .await
            .eval("console.log('one', {n: 1}); 'two'")
            .await;
        assert_eq!(&*outcome.output, "one {\n  \"n\": 1\n}\ntwo");
    }

    #[tokio::test]
    async fn thrown_error_is_an_error_outcome_with_the_logs() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = script(dir.path())
            .await
            .eval("console.log('before'); throw new Error('boom')")
            .await;
        assert!(outcome.is_error);
        assert!(outcome.output.starts_with("before\nError: boom"));
    }

    #[tokio::test]
    async fn shell_calls_run_in_parallel() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let outcome = script(dir.path())
            .await
            .eval("(await Promise.all([bash`sleep 1; echo a`, bash`sleep 1; echo b`])).map(r => r.output).join('')")
            .await;
        assert_eq!(outcome, Outcome::ok("a\nb\n"));
        assert!(started.elapsed() < Duration::from_millis(1800));
    }

    #[tokio::test]
    async fn template_values_are_shell_quoted() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = script(dir.path())
            .await
            .eval(r#"const name = "it's $(whoami)"; (await bash`printf %s ${name}`).output"#)
            .await;
        assert_eq!(outcome, Outcome::ok("it's $(whoami)"));
    }

    #[tokio::test]
    async fn read_then_edit_by_line_changes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "let tries = 3;\nlet x = 1;\n").unwrap();
        let outcome = script(dir.path())
            .await
            .eval(
                "const file = await read('a.rs');
                 const line = file.lines.find(l => l.includes('tries'));
                 (await edit('a.rs', [{op: 'replace', from: line, lines: ['let tries = 5;']}])).total",
            )
            .await;
        assert_eq!(outcome, Outcome::ok("2"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.rs")).unwrap(),
            "let tries = 5;\nlet x = 1;\n"
        );
    }

    #[tokio::test]
    async fn failed_call_throws_into_the_script() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = script(dir.path())
            .await
            .eval("try { await read('missing.txt') } catch (e) { 'caught ' + e.message }")
            .await;
        assert!(
            outcome
                .output
                .starts_with("caught could not read missing.txt")
        );
    }

    #[tokio::test]
    async fn busy_loop_is_stopped_and_the_sandbox_survives() {
        let dir = tempfile::tempdir().unwrap();
        let script = script(dir.path()).await;
        script.eval("let kept = 7").await;
        let outcome = script.eval("while (true) {}").await;
        assert!(outcome.is_error);
        assert!(outcome.output.contains("without awaiting a call"));
        assert_eq!(script.eval("kept").await, Outcome::ok("7"));
    }

    #[tokio::test]
    async fn unawaited_calls_finish_before_the_run_returns() {
        let dir = tempfile::tempdir().unwrap();
        let script = script(dir.path()).await;
        script.eval("bash`sleep 0.3; touch done`; 'returned'").await;
        assert!(dir.path().join("done").exists());
    }
}
