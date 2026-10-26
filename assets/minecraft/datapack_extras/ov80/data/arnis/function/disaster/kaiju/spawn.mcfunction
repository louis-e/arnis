execute store result score #py arnis.ds run data get entity @s Pos[1]
scoreboard players set #found arnis.ds 0
scoreboard players set #try arnis.ds 0
function arnis:disaster/kaiju/attempt
execute if score #found arnis.ds matches 0 summon marker run function arnis:disaster/kaiju/anywhere
