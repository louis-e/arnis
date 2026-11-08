$data modify storage arnis:walk p set from storage arnis:walk seg[$(s)].p[$(i)]
function arnis:walk/snap with storage arnis:walk p
# A light to wait for, or a kerb to look along, before going on.
execute if data storage arnis:walk p.g run function arnis:walk/light
execute if score @s arnis.ww matches 1.. run return 0
execute if data storage arnis:walk p.c run function arnis:walk/kerb
execute if score @s arnis.ww matches 1.. run return 0
# Now and then someone stops for a moment.
execute store result score #r arnis.walk run random value 0..39
execute if score #r arnis.walk matches 0 store result score @s arnis.ww run random value 15..50
execute if score @s arnis.ww matches 1.. run return 0
function arnis:walk/next
