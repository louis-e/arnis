scoreboard players add #tw_tk arnis.ds 1
scoreboard players operation #m4 arnis.ds = #tw_tk arnis.ds
scoreboard players operation #m4 arnis.ds %= #4 arnis
scoreboard players operation #m10 arnis.ds = #tw_tk arnis.ds
scoreboard players operation #m10 arnis.ds %= #10 arnis
execute as @e[type=armor_stand,tag=arnis.ds.tw] at @s run function arnis:disaster/tornado/core
function arnis:disaster/util/wind_tick
execute as @a[tag=!arnis.ds.inside,scores={arnis.ds.t=1..}] run scoreboard players set @s arnis.ds.t 0
tag @a remove arnis.ds.inside
execute if score #tw_tk arnis.ds matches 60.. run function arnis:disaster/tornado/regoal
