# Each entity keeps the run it was spawned in, which outlasts the time it spends unloaded.
execute as @e[tag=arnis.ob.ent] unless score @s arnis.ob.run matches -2147483648.. run scoreboard players operation @s arnis.ob.run = #run arnis.ob
execute as @e[tag=arnis.ob.ent] unless score @s arnis.ob.run = #run arnis.ob run kill @s
