scoreboard players set #lo arnis.ds 65
scoreboard players set #hi arnis.ds 85
function arnis:disaster/util/scatter
# Invisible, but on the locator bar.
execute at @s run summon armor_stand ~ ~ ~ {Tags:["arnis.ds.ent","arnis.ds.tw","arnis.ds.new"],Invisible:1b,Marker:1b,NoGravity:1b,Invulnerable:1b,Silent:1b,attributes:[{id:"minecraft:waypoint_transmit_range",base:2000.0d}]}
execute at @s run summon marker ~ ~ ~ {Tags:["arnis.ds.ent","arnis.ds.tw_goal","arnis.ds.new"]}
scoreboard players operation @e[tag=arnis.ds.new] arnis.ds.id = #tw_id arnis.ds
waypoint modify @e[type=armor_stand,tag=arnis.ds.new,limit=1] color red
tag @e[tag=arnis.ds.new] remove arnis.ds.new
kill @s
