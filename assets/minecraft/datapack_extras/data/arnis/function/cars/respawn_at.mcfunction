# Near the edge of what a player can see, within their reach, and with room on the road.
$execute positioned $(x) $(y) $(z) unless loaded ~ ~ ~ run return 0
$execute positioned $(x) $(y) $(z) if entity @a[distance=..144] run return 0
$execute positioned $(x) $(y) $(z) unless entity @a[distance=..168] run return 0
$execute positioned $(x) $(y) $(z) if entity @e[type=marker,tag=arnis.car,distance=..12] run return 0
execute store result storage arnis:cars spawn.p int 1 run random value 880..1120
function arnis:cars/respawn_kind
function arnis:cars/respawn_count with storage arnis:cars spawn
function arnis:cars/respawn_colour with storage arnis:cars spawn
$summon marker $(x) $(y) $(z) {Tags:["arnis.car","arnis.car.new"],data:{s:$(s),i:$(i),c:0,w:0,q:0,b:0b}}
function arnis:cars/respawn_body with storage arnis:cars spawn
execute as @e[type=marker,tag=arnis.car.new] run function arnis:cars/respawn_settle
scoreboard players add #near arnis.car 1
