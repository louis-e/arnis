# Near the edge of what a player can see, within their reach, and with room on the road.
# Where the chunks that far out are not loaded, as at a short simulation distance, nearer.
$execute positioned $(x) $(y) $(z) unless loaded ~ ~ ~ run return 0
$execute if score #far arnis.car matches 1 positioned $(x) $(y) $(z) if entity @a[distance=..144] run return 0
$execute positioned $(x) $(y) $(z) if entity @a[distance=..72] run return 0
$execute positioned $(x) $(y) $(z) unless entity @a[distance=..168] run return 0
$execute positioned $(x) $(y) $(z) if entity @e[type=marker,tag=arnis.car,distance=..12] run return 0
execute store result storage arnis:cars spawn.p int 1 run random value 880..1120
function arnis:cars/respawn_kind
function arnis:cars/respawn_count with storage arnis:cars spawn
function arnis:cars/respawn_colour with storage arnis:cars spawn
$summon marker $(x) $(y) $(z) {Tags:["arnis.car","arnis.car.new"],data:{s:$(s),i:$(i),c:0,w:0,q:0,b:0b}}
execute if data storage arnis:cars spawn{long:1b} run tag @e[type=marker,tag=arnis.car.new] add arnis.car.long
function arnis:cars/respawn_body with storage arnis:cars spawn
execute as @e[type=marker,tag=arnis.car.new] run function arnis:cars/respawn_settle
scoreboard players add #near arnis.car 1
