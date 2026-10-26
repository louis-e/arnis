scoreboard players set #len arnis.ds 40
scoreboard players set #rate arnis.ds 6
execute if score #pass arnis.ds matches 2.. run scoreboard players set #len arnis.ds 50
execute if score #pass arnis.ds matches 2.. run scoreboard players set #rate arnis.ds 4
scoreboard players operation #cd arnis.ds = #rate arnis.ds
scoreboard players remove #cd arnis.ds 2
