//! Lua scripting (Luau, sandboxed). A script defines hook functions the game
//! calls (`on_start`, `on_tick(dt)`, `on_arrive(stop)`, `on_depart(stop,
//! info)`, `on_complete(score)`) and acts through the `mission` table:
//!
//! ```lua
//! function on_depart(stop, info)
//!   if stop == "park" then
//!     mission.add_passengers("mill", 12)
//!     mission.message("A school group is waiting at Mill Road.")
//!   end
//! end
//! ```
//!
//! Scripts are deterministic: no clock, no files, no `os`; `math.random`
//! draws from the simulation's seeded generator. They only queue commands;
//! the game applies them after each hook.

use mlua::{Function, Lua, LuaOptions, StdLib, Table, Value};
use std::cell::RefCell;
use std::rc::Rc;
use wp_core::SimRng;

/// What a script asked for, applied by the game after the hook returns.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Replace the objective text shown to the player.
    Objective(String),
    /// Show a message to the player.
    Message(String),
    /// More people wait at a stop.
    AddPassengers {
        stop: String,
        count: u32,
    },
    /// Extra points (or a penalty if negative) on the score.
    Bonus(i32),
    Fail(String),
    /// A line for the log (`wp test` output, agents).
    Log(String),
}

/// Read-only facts a script can look at (`mission.state`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// Seconds since the start.
    pub time_s: f32,
    pub speed_kmh: f32,
    pub on_board: u32,
    /// Id of the stop being driven to.
    pub next_stop: String,
    pub stops_served: u32,
}

/// A script error, with the line when known.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptError {
    pub line: Option<u32>,
    pub message: String,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "line {l}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

fn error(e: mlua::Error) -> ScriptError {
    let text = e.to_string();
    // Luau messages look like `[string "name"]:12: message`.
    let line = text
        .split("]:")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .and_then(|n| n.trim().parse().ok());
    let message = match line {
        Some(_) => text
            .split("]:")
            .nth(1)
            .and_then(|rest| rest.split_once(':'))
            .map(|(_, m)| m.trim().to_string())
            .unwrap_or(text.clone()),
        None => text.clone(),
    };
    ScriptError {
        line,
        message: message.lines().next().unwrap_or("").to_string(),
    }
}

/// A loaded script.
pub struct Script {
    lua: Lua,
    commands: Rc<RefCell<Vec<Command>>>,
    view: Rc<RefCell<View>>,
}

/// Check that `source` compiles, without running it.
pub fn check(source: &str, name: &str) -> Result<(), ScriptError> {
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::default()).map_err(error)?;
    lua.load(source)
        .set_name(name)
        .into_function()
        .map(|_| ())
        .map_err(error)
}

impl Script {
    /// Load and run the script's top level (defining its hooks). `seed`
    /// seeds `math.random`.
    pub fn load(source: &str, name: &str, seed: u64) -> Result<Script, ScriptError> {
        let lua = Lua::new_with(
            StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::UTF8 | StdLib::BIT,
            LuaOptions::default(),
        )
        .map_err(error)?;
        let commands: Rc<RefCell<Vec<Command>>> = Rc::default();
        let view: Rc<RefCell<View>> = Rc::default();
        Self::install_api(&lua, &commands, &view, seed).map_err(error)?;
        lua.sandbox(true).map_err(error)?;
        lua.load(source).set_name(name).exec().map_err(error)?;
        Ok(Script {
            lua,
            commands,
            view,
        })
    }

    fn install_api(
        lua: &Lua,
        commands: &Rc<RefCell<Vec<Command>>>,
        view: &Rc<RefCell<View>>,
        seed: u64,
    ) -> mlua::Result<()> {
        let mission = lua.create_table()?;
        let push = |c: &Rc<RefCell<Vec<Command>>>| c.clone();
        let c = push(commands);
        mission.set(
            "objective",
            lua.create_function(move |_, text: String| {
                c.borrow_mut().push(Command::Objective(text));
                Ok(())
            })?,
        )?;
        let c = push(commands);
        mission.set(
            "message",
            lua.create_function(move |_, text: String| {
                c.borrow_mut().push(Command::Message(text));
                Ok(())
            })?,
        )?;
        let c = push(commands);
        mission.set(
            "add_passengers",
            lua.create_function(move |_, (stop, count): (String, u32)| {
                c.borrow_mut().push(Command::AddPassengers { stop, count });
                Ok(())
            })?,
        )?;
        let c = push(commands);
        mission.set(
            "bonus",
            lua.create_function(move |_, points: i32| {
                c.borrow_mut().push(Command::Bonus(points));
                Ok(())
            })?,
        )?;
        let c = push(commands);
        mission.set(
            "fail",
            lua.create_function(move |_, reason: String| {
                c.borrow_mut().push(Command::Fail(reason));
                Ok(())
            })?,
        )?;
        let c = push(commands);
        mission.set(
            "log",
            lua.create_function(move |_, text: String| {
                c.borrow_mut().push(Command::Log(text));
                Ok(())
            })?,
        )?;
        let v = view.clone();
        mission.set(
            "state",
            lua.create_function(move |lua, ()| {
                let v = v.borrow();
                let t = lua.create_table()?;
                t.set("time_s", v.time_s)?;
                t.set("speed_kmh", v.speed_kmh)?;
                t.set("on_board", v.on_board)?;
                t.set("next_stop", v.next_stop.clone())?;
                t.set("stops_served", v.stops_served)?;
                Ok(t)
            })?,
        )?;
        lua.globals().set("mission", mission)?;
        // Deterministic random numbers from the simulation's generator.
        let rng = Rc::new(RefCell::new(SimRng::fork(seed, "script")));
        let math: Table = lua.globals().get("math")?;
        math.set(
            "random",
            lua.create_function(move |_, (a, b): (Option<i64>, Option<i64>)| {
                let mut r = rng.borrow_mut();
                Ok(match (a, b) {
                    (None, _) => Value::Number(r.next_f32() as f64),
                    (Some(hi), None) => {
                        Value::Integer(1 + (r.next_u64() % hi.max(1) as u64) as i64)
                    }
                    (Some(lo), Some(hi)) => {
                        let span = (hi - lo + 1).max(1) as u64;
                        Value::Integer(lo + (r.next_u64() % span) as i64)
                    }
                })
            })?,
        )?;
        math.set("randomseed", lua.create_function(|_, _: Value| Ok(()))?)?;
        Ok(())
    }

    /// Update what `mission.state()` returns.
    pub fn set_view(&self, view: View) {
        *self.view.borrow_mut() = view;
    }

    /// Call hook `name` with `args`, if the script defines it.
    pub fn call(&self, name: &str, args: impl mlua::IntoLuaMulti) -> Result<(), ScriptError> {
        let hook: Value = self.lua.globals().get(name).map_err(error)?;
        match hook {
            Value::Function(f) => f.call::<()>(args).map_err(error),
            _ => Ok(()),
        }
    }

    /// Call a hook with a table of named numbers (e.g. departure details).
    pub fn call_with_info(
        &self,
        name: &str,
        first: &str,
        info: &[(&str, f64)],
    ) -> Result<(), ScriptError> {
        let t = self.lua.create_table().map_err(error)?;
        for (k, v) in info {
            t.set(*k, *v).map_err(error)?;
        }
        self.call(name, (first.to_string(), t))
    }

    /// The commands queued since the last call.
    pub fn take_commands(&self) -> Vec<Command> {
        std::mem::take(&mut *self.commands.borrow_mut())
    }

    /// Whether the script defines hook `name`.
    pub fn has(&self, name: &str) -> bool {
        self.lua.globals().get::<Function>(name).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_queue_commands() {
        let s = Script::load(
            r#"
            local count = 0
            function on_arrive(stop)
              count += 1
              mission.message("arrived at " .. stop .. " #" .. count)
              if stop == "park" then mission.add_passengers("mill", 12) end
            end
            function on_depart(stop, info)
              if info.left_behind > 0 then mission.bonus(-5) end
            end
            "#,
            "test.lua",
            1,
        )
        .unwrap();
        s.call("on_arrive", "park".to_string()).unwrap();
        s.call_with_info("on_depart", "park", &[("left_behind", 2.0)])
            .unwrap();
        s.call("on_missing_hook", ()).unwrap();
        assert_eq!(
            s.take_commands(),
            vec![
                Command::Message("arrived at park #1".into()),
                Command::AddPassengers {
                    stop: "mill".into(),
                    count: 12
                },
                Command::Bonus(-5),
            ]
        );
    }

    #[test]
    fn errors_have_lines_and_no_os_access() {
        let e = check("local x = = 1", "bad.lua").unwrap_err();
        assert_eq!(e.line, Some(1), "{e}");
        let e = Script::load("x = 1\nos.exit()", "os.lua", 1).err().unwrap();
        assert_eq!(e.line, Some(2), "{e}");
        let s = Script::load("function on_tick() error('boom') end", "err.lua", 1).unwrap();
        let e = s.call("on_tick", 0.016).unwrap_err();
        assert!(e.message.contains("boom"), "{e}");
    }

    #[test]
    fn random_numbers_are_seeded() {
        let roll = |seed| {
            let s = Script::load(
                "function on_start() mission.log(tostring(math.random(1, 1000))) end",
                "r.lua",
                seed,
            )
            .unwrap();
            s.call("on_start", ()).unwrap();
            s.take_commands()
        };
        assert_eq!(roll(7), roll(7));
        assert_ne!(roll(7), roll(8));
    }

    #[test]
    fn state_is_readable() {
        let s = Script::load(
            "function on_tick() local st = mission.state(); mission.log(st.next_stop .. ' ' .. st.on_board) end",
            "s.lua",
            1,
        )
        .unwrap();
        s.set_view(View {
            next_stop: "park".into(),
            on_board: 9,
            ..Default::default()
        });
        s.call("on_tick", 0.016).unwrap();
        assert_eq!(s.take_commands(), vec![Command::Log("park 9".into())]);
    }
}
