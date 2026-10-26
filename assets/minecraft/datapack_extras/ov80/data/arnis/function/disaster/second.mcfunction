scoreboard players set #tk arnis.ds 0
execute if score #state arnis.ds matches 0 if entity @a run function arnis:disaster/start
execute if score #state arnis.ds matches 1 run function arnis:disaster/running
# What a stopped or restarted run left behind is cleared as its chunks load.
execute unless score #state arnis.ds matches 1 as @e[type=marker,tag=arnis.ds.gust] at @s run function arnis:disaster/util/gust_end
execute unless score #state arnis.ds matches 1 run kill @e[tag=arnis.ds.ent]
execute if score #state arnis.ds matches 1 run function arnis:disaster/sweep
function arnis:disaster/water/second
