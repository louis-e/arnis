scoreboard players add #run arnis.ds 1
execute unless data storage arnis:ds bag[0] run function arnis:disaster/bag/refill
execute store result score #n arnis.ds run data get storage arnis:ds bag
scoreboard players remove #n arnis.ds 1
scoreboard players set #i arnis.ds 0
execute if score #n arnis.ds matches 1.. store result storage arnis:ds pick.m int 1 run scoreboard players get #n arnis.ds
execute if score #n arnis.ds matches 1.. run function arnis:disaster/bag/roll with storage arnis:ds pick
execute store result storage arnis:ds pick.i int 1 run scoreboard players get #i arnis.ds
function arnis:disaster/bag/take with storage arnis:ds pick
scoreboard players set #phase arnis.ds 2
scoreboard players set #t arnis.ds 10
# Water takes longer to get away from.
execute if score #kind arnis.ds matches 7 run scoreboard players set #t arnis.ds 15
execute if score #kind arnis.ds matches 9 run scoreboard players set #t arnis.ds 15
execute if score #kind arnis.ds matches 11 run scoreboard players set #t arnis.ds 15
scoreboard players operation #warn_len arnis.ds = #t arnis.ds
function arnis:disaster/warn
