# Each entity keeps the round it was spawned in, which outlasts the time it spends unloaded.
execute as @e[tag=arnis.ds.ent] unless score @s arnis.ds.run matches -2147483648.. run scoreboard players operation @s arnis.ds.run = #run arnis.ds
execute as @e[type=marker,tag=arnis.ds.gust] unless score @s arnis.ds.run = #run arnis.ds at @s run function arnis:disaster/util/gust_end
execute as @e[tag=arnis.ds.ent] unless score @s arnis.ds.run = #run arnis.ds run kill @s
