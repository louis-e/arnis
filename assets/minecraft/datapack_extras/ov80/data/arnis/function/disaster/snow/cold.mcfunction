# Out in it, the cold gets to you; leather keeps it off, as it does in powder snow.
function arnis:disaster/util/outdoors
execute if score #out arnis.ds matches 0 run return 0
effect give @s minecraft:slowness 2 0 true
execute if items entity @s armor.* #minecraft:freeze_immune_wearables run return 0
scoreboard players operation #q arnis.ds = #t arnis.ds
scoreboard players operation #q arnis.ds %= #3 arnis
execute if score #pass arnis.ds matches 2.. run scoreboard players operation #q arnis.ds %= #2 arnis
execute if score #q arnis.ds matches 0 run damage @s 1 minecraft:freeze
