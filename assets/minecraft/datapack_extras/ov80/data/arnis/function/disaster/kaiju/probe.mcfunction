execute store result score #c arnis.ds run data get entity @s Pos[1]
scoreboard players operation #c arnis.ds -= #gy arnis.ds
execute if score #c arnis.ds matches ..0 run scoreboard players set #c arnis.ds 0
scoreboard players operation #c arnis.ds *= #3 arnis
$scoreboard players add #c arnis.ds $(p)
$execute if score #c arnis.ds < #best arnis.ds run scoreboard players set #ba arnis.ds $(a)
execute if score #c arnis.ds < #best arnis.ds run scoreboard players operation #best arnis.ds = #c arnis.ds
kill @s
