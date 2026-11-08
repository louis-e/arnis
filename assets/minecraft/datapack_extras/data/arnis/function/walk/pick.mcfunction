# One of a list in the wardrobe, at random, into pick.v.
$execute store result score #n arnis.walk if data storage arnis:walk wear.$(list)[]
execute store result score #k arnis.walk run random value 0..999
scoreboard players operation #k arnis.walk %= #n arnis.walk
$data modify storage arnis:walk pick set value {list:"$(list)"}
execute store result storage arnis:walk pick.k int 1 run scoreboard players get #k arnis.walk
function arnis:walk/pick_at with storage arnis:walk pick
