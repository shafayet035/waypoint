-- Route 1 extras (Luau). Hooks: on_start, on_tick(dt), on_arrive(stop),
-- on_depart(stop, info), on_complete(score), on_fail(reason).
-- `mission` API: objective(text), message(text), add_passengers(stop, n),
-- bonus(points), fail(reason), log(text), state().

local laps = 0

function on_start()
  mission.message("Good morning! Route 1 leaves Station Avenue for the Station stop.")
end

function on_depart(stop, info)
  if stop == "market" then
    laps += 1
    mission.log("lap " .. laps .. " done")
  end
  -- On the second lap a school group waits at Mill Road.
  if stop == "park" and laps == 1 then
    mission.add_passengers("mill", 12)
    mission.message("A school group is waiting at Mill Road.")
  end
  if info.left_behind > 0 then
    mission.message(info.left_behind .. " people were left behind at " .. stop .. ".")
  end
end

function on_complete(score)
  if score >= 90 then
    mission.message("Excellent driving! Bonus: 5 points.")
    mission.bonus(5)
  else
    mission.message("Route complete. Score " .. score .. ".")
  end
end
