scoreboard players operation #restore arnis.ob = #night_t arnis.ob
scoreboard players operation #restore arnis.ob *= #20 arnis
scoreboard players add #restore arnis.ob 13000
execute store result storage arnis:ob night.t int 1 run scoreboard players get #restore arnis.ob
function arnis:outbreak/time_set with storage arnis:ob night
tellraw @a {"text":"You can't sleep through an outbreak.","color":"gray"}
