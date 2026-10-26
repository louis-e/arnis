scoreboard players enable @a arnis.ds.ctl
execute as @e[type=marker,tag=arnis.ds.gust] at @s run function arnis:disaster/util/gust_tick
execute as @a[scores={arnis.ds.ctl=1..}] at @s run function arnis:disaster/control
execute if score #state arnis.ds matches 1 if score #phase arnis.ds matches 2 run function arnis:disaster/warn_tick
execute if score #state arnis.ds matches 1 if score #phase arnis.ds matches 3 run function arnis:disaster/active_tick
function arnis:disaster/water/tick
scoreboard players add #tk arnis.ds 1
execute if score #tk arnis.ds matches 20.. run function arnis:disaster/second
