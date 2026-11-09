scoreboard objectives add arnis.walk dummy
# Each walker's lane, the point it heads for, its lane's length, steps to go, pace,
# steps it still waits and steps it has been held up.
scoreboard objectives add arnis.ws dummy
scoreboard objectives add arnis.wi dummy
scoreboard objectives add arnis.wl dummy
scoreboard objectives add arnis.wn dummy
scoreboard objectives add arnis.wv dummy
scoreboard objectives add arnis.ww dummy
scoreboard objectives add arnis.wb dummy
# Each walker's own run of chances, shared with whoever walks with them, own pace,
# heading and number: of two held up by each other, the younger waits.
scoreboard objectives add arnis.wr dummy
scoreboard objectives add arnis.wq dummy
scoreboard objectives add arnis.wy dummy
scoreboard objectives add arnis.wk dummy
scoreboard players set #lcg_a arnis.walk 1103515245
scoreboard players set #lcg_c arnis.walk 12345
scoreboard players set #65536 arnis.walk 65536
scoreboard players set #40 arnis.walk 40
scoreboard players set #36 arnis.walk 36
scoreboard players set #360 arnis.walk 360
scoreboard players set #walk_t arnis 0
# Half a second after the cars, so the two never add new ones in the same tick.
scoreboard players set #walk_r arnis 5
scoreboard players set #-1 arnis -1
scoreboard players set #2 arnis 2
scoreboard players set #41 arnis 41
scoreboard players set #100 arnis 100
# The game's own default looks, broad and slim.
data modify storage arnis:walk skins set value [{t:"minecraft:entity/player/wide/steve",m:"wide"},{t:"minecraft:entity/player/wide/alex",m:"wide"},{t:"minecraft:entity/player/wide/ari",m:"wide"},{t:"minecraft:entity/player/wide/efe",m:"wide"},{t:"minecraft:entity/player/wide/kai",m:"wide"},{t:"minecraft:entity/player/wide/makena",m:"wide"},{t:"minecraft:entity/player/wide/noor",m:"wide"},{t:"minecraft:entity/player/wide/sunny",m:"wide"},{t:"minecraft:entity/player/wide/zuri",m:"wide"},{t:"minecraft:entity/player/slim/steve",m:"slim"},{t:"minecraft:entity/player/slim/alex",m:"slim"},{t:"minecraft:entity/player/slim/ari",m:"slim"},{t:"minecraft:entity/player/slim/efe",m:"slim"},{t:"minecraft:entity/player/slim/kai",m:"slim"},{t:"minecraft:entity/player/slim/makena",m:"slim"},{t:"minecraft:entity/player/slim/noor",m:"slim"},{t:"minecraft:entity/player/slim/sunny",m:"slim"},{t:"minecraft:entity/player/slim/zuri",m:"slim"}]
# What people wear over the skin, as dyed leather: tops, trousers, shoes and caps, and the
# bags some carry. Villagers before 1.21.9 come from a region and a trade instead.
data modify storage arnis:walk wear set value {tops:[I;2042436,1842204,3815994,9079434,15132390,13153418,5988918,7216939,11546672,13214247,3037754,8366033,3890060,7031343,14258086,3112831],bottoms:[I;3099246,2043722,1842204,5592405,11902837,2306122,5914672,7311285],shoes:[I;1710618,4861984,15132390,7237230,2306122,10516560],caps:[I;11546672,2306122,1842204,15132390,5988918,13153418],bags:["minecraft:bundle","minecraft:black_bundle","minecraft:red_bundle","minecraft:blue_bundle","minecraft:gray_bundle","minecraft:green_bundle","minecraft:brown_bundle","minecraft:white_bundle","minecraft:pink_bundle","minecraft:yellow_bundle"],types:["minecraft:plains","minecraft:desert","minecraft:jungle","minecraft:savanna","minecraft:snow","minecraft:swamp","minecraft:taiga"],jobs:["minecraft:none","minecraft:none","minecraft:none","minecraft:nitwit","minecraft:farmer","minecraft:fisherman","minecraft:librarian","minecraft:cartographer","minecraft:cleric","minecraft:fletcher","minecraft:shepherd","minecraft:toolsmith","minecraft:weaponsmith","minecraft:butcher","minecraft:leatherworker","minecraft:mason"]}
function arnis:walk/network
execute store result score #lanes arnis.walk run data get storage arnis:walk seg
