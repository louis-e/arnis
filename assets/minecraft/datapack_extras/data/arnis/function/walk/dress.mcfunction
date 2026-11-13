# Each piece by its own chance: a top, trousers, shoes, a cap, a bag in hand.
data modify storage arnis:walk spawn.eq set value {}
execute store result score #r arnis.walk run random value 0..99
execute if score #r arnis.walk matches ..54 run function arnis:walk/wear {slot:"chest",item:"leather_chestplate",list:"tops"}
execute store result score #r arnis.walk run random value 0..99
execute if score #r arnis.walk matches ..64 run function arnis:walk/wear {slot:"legs",item:"leather_leggings",list:"bottoms"}
execute store result score #r arnis.walk run random value 0..99
execute if score #r arnis.walk matches ..44 run function arnis:walk/wear {slot:"feet",item:"leather_boots",list:"shoes"}
execute store result score #r arnis.walk run random value 0..99
execute if score #r arnis.walk matches ..9 run function arnis:walk/wear {slot:"head",item:"leather_helmet",list:"caps"}
execute store result score #r arnis.walk run random value 0..99
execute if score #r arnis.walk matches ..14 run function arnis:walk/carry
function arnis:walk/pick {list:"types"}
data modify storage arnis:walk spawn.vt set from storage arnis:walk pick.v
function arnis:walk/pick {list:"jobs"}
data modify storage arnis:walk spawn.vp set from storage arnis:walk pick.v
