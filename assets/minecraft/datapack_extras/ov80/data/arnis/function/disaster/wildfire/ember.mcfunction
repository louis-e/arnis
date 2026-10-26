scoreboard players set #lo arnis.ds 4
scoreboard players set #hi arnis.ds 18
function arnis:disaster/util/scatter
execute at @s run summon small_fireball ~ ~12 ~ {Motion:[0.0d,-0.6d,0.0d],acceleration_power:0.02d,Tags:["arnis.ds.ent"]}
kill @s
