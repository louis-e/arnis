kill @e[type=armor_stand,tag=arnis.ds.tw]
kill @e[type=marker,tag=arnis.ds.tw_goal]
kill @e[type=block_display,tag=arnis.ds.debris]
effect clear @a minecraft:levitation
scoreboard players set @a arnis.ds.t 0
weather clear
function arnis:disaster/util/wind_stop
