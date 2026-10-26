# Heads for whoever is nearest, never quite straight at them.
execute if score @s arnis.ds.id < #tw_min arnis.ds run return run kill @s
scoreboard players operation #id arnis.ds = @s arnis.ds.id
execute as @e[type=armor_stand,tag=arnis.ds.tw] if score @s arnis.ds.id = #id arnis.ds run tag @s add arnis.ds.cur
execute at @e[type=armor_stand,tag=arnis.ds.cur,limit=1] run tp @s @p[gamemode=!spectator,distance=..250]
tag @e[type=armor_stand,tag=arnis.ds.cur] remove arnis.ds.cur
scoreboard players set #lo arnis.ds 1
scoreboard players set #hi arnis.ds 20
function arnis:disaster/util/scatter
