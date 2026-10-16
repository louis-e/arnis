# At the start of a lane, out of sight of anyone close but near enough to someone to be worth it.
$execute positioned $(x) $(y) $(z) unless loaded ~ ~ ~ run return 0
$execute positioned $(x) $(y) $(z) if entity @a[distance=..40] run return 0
$execute positioned $(x) $(y) $(z) unless entity @a[distance=..96] run return 0
$execute positioned $(x) $(y) $(z) if entity @e[type=marker,tag=arnis.car,distance=..12] run return 0
execute store result storage arnis:cars spawn.p int 1 run random value 880..1120
execute store result score #r arnis.car run random value 0..99
data modify storage arnis:cars spawn.kind set value "sedan"
execute if score #r arnis.car matches 60..85 run data modify storage arnis:cars spawn.kind set value "hatchback"
execute if score #r arnis.car matches 86.. run data modify storage arnis:cars spawn.kind set value "van"
execute store result score #n arnis.car run data get storage arnis:cars colours.car
execute if score #r arnis.car matches 86.. store result score #n arnis.car run data get storage arnis:cars colours.van
execute store result score #r arnis.car run random value 0..9999
scoreboard players operation #r arnis.car %= #n arnis.car
execute store result storage arnis:cars spawn.k int 1 run scoreboard players get #r arnis.car
function arnis:cars/respawn_colour with storage arnis:cars spawn
$summon marker $(x) $(y) $(z) {Tags:["arnis.car","arnis.car.new"],data:{s:$(s),i:1,c:0,w:0,q:0,b:0b}}
function arnis:cars/respawn_body with storage arnis:cars spawn
execute as @e[type=marker,tag=arnis.car.new] run function arnis:cars/respawn_settle
scoreboard players remove #pending arnis.car 1
