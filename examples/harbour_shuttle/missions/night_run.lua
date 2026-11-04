-- Night Run extras (Luau). `wp docs scripting` lists the hooks and the API.

local laps = 0

function on_start()
  mission.message("Evening service. Mind the wet cobbles on Ferry Hill.")
end

function on_depart(stop, info)
  if stop == "upper_town" then
    laps += 1
    mission.log("lap " .. laps .. " done")
    if laps == 1 then
      -- The last ferry has docked: a crowd waits at the terminal.
      mission.add_passengers("ferry_terminal", 10)
      mission.message("The last ferry is in. Ten more waiting at the terminal.")
    end
  end
  if info.left_behind > 0 then
    mission.message(info.left_behind .. " left behind at " .. stop .. ". The next shuttle is an hour away.")
  end
end

function on_complete(score)
  if score >= 85 then
    mission.message("Smooth and on time in the rain. Bonus: 5 points.")
    mission.bonus(5)
  else
    mission.message("Night run complete. Score " .. score .. ".")
  end
end
