execute as @e[type=marker,tag=arnis.ds.gust] at @s run function arnis:disaster/util/gust_end
kill @e[tag=arnis.ds.ent]
# Flooded ground drains as players come near it again.
scoreboard players set #wv_on arnis.ds 0
scoreboard players set #lv_flood arnis.ds 0
scoreboard players set #lv_tsunami arnis.ds 0
scoreboard players set #lv_snow arnis.ds 0
scoreboard players set #wd_snow arnis.ds 1
function arnis:disaster/util/wind_stop
function arnis:disaster/flood/tops
function arnis:disaster/tsunami/tops
effect clear @a minecraft:levitation
weather clear
